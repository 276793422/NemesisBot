//! Additional tests for http_provider.rs focusing on uncovered code paths.
//!
//! Uses wiremock to test actual HTTP request/response handling, including:
//! - Request body construction (URL, headers, JSON body)
//! - Response parsing (success, partial, malformed JSON)
//! - Streaming SSE parsing (content, tool calls, reasoning, [DONE])
//! - Error mapping (4xx, 5xx, malformed body)
//! - Authentication header construction (Bearer token + custom headers)
//! - Model name normalization edge cases
//! - Tool call accumulation in streaming
//! - extract_usage() helper for various provider formats

use super::*;
use crate::types::*;
use std::collections::HashMap;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helper: minimal user message
// ---------------------------------------------------------------------------

fn user_message(content: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: content.to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }
}

fn basic_config(base_url: String) -> HttpProviderConfig {
    HttpProviderConfig {
        name: "test".to_string(),
        base_url,
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 10,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }
}

// ---------------------------------------------------------------------------
// Tests for extract_usage() helper
// ---------------------------------------------------------------------------

#[test]
fn test_extract_usage_deepseek_with_hit_and_miss() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 100,
        "completion_tokens": 50,
        "total_tokens": 150,
        "prompt_cache_hit_tokens": 30,
        "prompt_cache_miss_tokens": 70
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, Some(30));
    assert_eq!(usage.cache_creation_tokens, Some(70));
    assert_eq!(usage.cache_read_tokens, Some(30));
}

#[test]
fn test_extract_usage_deepseek_hit_only() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 100,
        "completion_tokens": 50,
        "total_tokens": 150,
        "prompt_cache_hit_tokens": 40
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, Some(40));
    assert_eq!(usage.cache_read_tokens, Some(40));
    assert_eq!(usage.cache_creation_tokens, None);
}

#[test]
fn test_extract_usage_openai_with_negative_miss() {
    // cached > prompt_tokens should clamp to 0 (negative miss scenario)
    let usage_json = serde_json::json!({
        "prompt_tokens": 5,
        "completion_tokens": 1,
        "total_tokens": 6,
        "prompt_tokens_details": {
            "cached_tokens": 10
        }
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, Some(10));
    // miss = max(5 - 10, 0) = 0
    assert_eq!(usage.cache_creation_tokens, Some(0));
    assert_eq!(usage.cache_read_tokens, Some(10));
}

#[test]
fn test_extract_usage_anthropic_creation_only() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 100,
        "completion_tokens": 50,
        "total_tokens": 150,
        "cache_creation_input_tokens": 20
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, None);
    assert_eq!(usage.cache_creation_tokens, Some(20));
    assert_eq!(usage.cache_read_tokens, None);
}

#[test]
fn test_extract_usage_anthropic_read_only() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 100,
        "completion_tokens": 50,
        "total_tokens": 150,
        "cache_read_input_tokens": 15
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, None);
    assert_eq!(usage.cache_creation_tokens, None);
    assert_eq!(usage.cache_read_tokens, Some(15));
}

#[test]
fn test_extract_usage_anthropic_both() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 100,
        "completion_tokens": 50,
        "total_tokens": 150,
        "cache_creation_input_tokens": 25,
        "cache_read_input_tokens": 35
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cache_creation_tokens, Some(25));
    assert_eq!(usage.cache_read_tokens, Some(35));
}

#[test]
fn test_extract_usage_no_cache_info() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "total_tokens": 15
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, None);
    assert_eq!(usage.cache_creation_tokens, None);
    assert_eq!(usage.cache_read_tokens, None);
}

#[test]
fn test_extract_usage_null_cached_tokens() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "total_tokens": 15,
        "prompt_tokens_details": {
            "cached_tokens": null
        }
    });
    let usage = extract_usage(&usage_json);
    assert_eq!(usage.cached_tokens, None);
}

// ---------------------------------------------------------------------------
// HTTP chat() success/error path tests with wiremock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_success_basic_response() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body = serde_json::json!({
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello, world!"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 3,
            "total_tokens": 8
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .expect("chat should succeed");

    assert_eq!(result.content, "Hello, world!");
    assert_eq!(result.finish_reason, "stop");
    assert_eq!(result.usage.as_ref().unwrap().prompt_tokens, 5);
    assert_eq!(result.usage.as_ref().unwrap().completion_tokens, 3);
    assert!(result.tool_calls.is_empty());
}

#[tokio::test]
async fn test_chat_success_with_reasoning_content() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Answer",
                "reasoning_content": "thinking..."
            },
            "finish_reason": "stop"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();

    assert_eq!(result.content, "Answer");
    assert_eq!(result.reasoning_content.as_deref(), Some("thinking..."));
}

#[tokio::test]
async fn test_chat_success_with_tool_calls() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":\"SF\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("weather?")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();

    assert_eq!(result.finish_reason, "tool_calls");
    assert_eq!(result.tool_calls.len(), 1);
    let tc = &result.tool_calls[0];
    assert_eq!(tc.id, "call_abc");
    assert_eq!(tc.call_type.as_deref(), Some("function"));
    assert_eq!(tc.function.as_ref().unwrap().name, "get_weather");
    assert_eq!(
        tc.function.as_ref().unwrap().arguments,
        "{\"location\":\"SF\"}"
    );
}

#[tokio::test]
async fn test_chat_tool_calls_filtered_when_id_or_function_missing() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    // The filter_map uses `?` on id/function.name/function.arguments — so any
    // entry missing these fields gets filtered out. Entries with empty-string
    // values still pass (Some("") is Some).
    let response_body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "skipped",
                "tool_calls": [
                    { "function": {} },
                    { "id": "x", "function": { "arguments": "{}" } },
                    { "id": "valid", "type": "function",
                      "function": { "name": "ok", "arguments": "{}" } }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();

    // First (no id), second (no function.name), third (full) -> third only passes filter
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].id, "valid");
    assert_eq!(result.tool_calls[0].function.as_ref().unwrap().name, "ok");
}

#[tokio::test]
async fn test_chat_finish_reason_defaults_to_stop() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Hi"
            }
            // finish_reason missing
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();

    assert_eq!(result.finish_reason, "stop");
}

#[tokio::test]
async fn test_chat_no_usage_when_field_absent() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body =
        serde_json::json!({"choices": [{"message": {"role": "assistant", "content": "x"}, "finish_reason": "stop"}]});

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();
    assert!(result.usage.is_none());
}

#[tokio::test]
async fn test_chat_content_defaults_to_empty_when_missing() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body =
        serde_json::json!({"choices": [{"message": {"role": "assistant"}, "finish_reason": "stop"}]});

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(result.content, "");
}

#[tokio::test]
async fn test_chat_error_401_auth() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid api key"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;

    match result {
        Err(FailoverError::Auth { status, .. }) => assert_eq!(status, 401),
        other => panic!("Expected Auth error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_chat_error_403_auth() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Auth { status: 403, .. })));
}

#[tokio::test]
async fn test_chat_error_429_rate_limit() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::RateLimit { .. })));
}

#[tokio::test]
async fn test_chat_error_402_billing() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_string("payment required"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Billing { .. })));
}

#[tokio::test]
async fn test_chat_error_502_overloaded() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Overloaded { .. })));
}

#[tokio::test]
async fn test_chat_error_503_overloaded() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Overloaded { .. })));
}

#[tokio::test]
async fn test_chat_error_500_unknown() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Unknown { .. })));
}

#[tokio::test]
async fn test_chat_error_400_unknown() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Unknown { .. })));
}

#[tokio::test]
async fn test_chat_malformed_json_returns_format_error() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not valid json {"))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(matches!(result, Err(FailoverError::Format { .. })));
}

// ---------------------------------------------------------------------------
// Header / auth construction tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_sends_bearer_auth_header() {
    let server = MockServer::start().await;
    let mut config = basic_config(server.uri());
    config.api_key = "sk-secret-123".to_string();
    let provider = HttpProvider::new(config);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer sk-secret-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_chat_sends_custom_headers() {
    let server = MockServer::start().await;
    let mut config = basic_config(server.uri());
    config.headers.insert("X-Trace-Id".to_string(), "abc-123".to_string());
    let provider = HttpProvider::new(config);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("X-Trace-Id", "abc-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_chat_sends_content_type_header() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_chat_raw_request_and_response_captured() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_json = serde_json::json!({
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();

    assert!(result.raw_request_body.is_some());
    assert_eq!(result.raw_request_body.unwrap()["model"], "gpt-4");
    assert!(result.raw_response_body.is_some());
    assert!(result.raw_response_body.unwrap().contains("ok"));
}

// ---------------------------------------------------------------------------
// chat_stream tests with wiremock SSE responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_stream_success_content_done() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\", world\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut received_content = String::new();
    let mut got_usage = false;
    let mut stop_count = 0;
    while let Some(chunk_result) = rx.recv().await {
        match chunk_result {
            Ok(chunk) => {
                received_content.push_str(&chunk.delta);
                if chunk.finish_reason.as_deref() == Some("stop") {
                    stop_count += 1;
                    if chunk.usage.is_some() {
                        got_usage = true;
                        assert_eq!(chunk.usage.as_ref().unwrap().total_tokens, 3);
                    }
                }
            }
            Err(e) => panic!("unexpected stream error: {:?}", e),
        }
    }
    assert_eq!(received_content, "Hello, world");
    // API emits stop+usage chunk, then [DONE] emits another synthetic stop chunk.
    assert_eq!(stop_count, 2);
    assert!(got_usage);
}

#[tokio::test]
async fn test_chat_stream_done_without_prior_content() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let sse_body = "data: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut count = 0;
    while let Some(Ok(chunk)) = rx.recv().await {
        assert!(chunk.delta.is_empty());
        assert_eq!(chunk.finish_reason.as_deref(), Some("stop"));
        count += 1;
    }
    assert_eq!(count, 1, "[DONE] alone should produce exactly one stop chunk");
}

#[tokio::test]
async fn test_chat_stream_with_reasoning_content() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking step 1. \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"step 2.\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"final answer\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut final_chunk: Option<StreamChunk> = None;
    while let Some(Ok(chunk)) = rx.recv().await {
        // Track the chunk that has actual content + finish_reason (the API's real finish chunk,
        // before [DONE] synthesizes an empty stop chunk).
        if chunk.finish_reason.is_some() && (!chunk.delta.is_empty() || chunk.usage.is_some()) {
            final_chunk = Some(chunk);
        }
    }
    let final_chunk = final_chunk.expect("should have a finish chunk with content");
    assert_eq!(
        final_chunk.reasoning_content.as_deref(),
        Some("thinking step 1. step 2.")
    );
    assert_eq!(final_chunk.delta, "final answer");
}

#[tokio::test]
async fn test_chat_stream_accumulates_tool_calls() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    // Simulate a streaming tool call split across chunks
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"loc\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"SF\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("weather?")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut got_tool_call_chunk = false;
    while let Some(Ok(chunk)) = rx.recv().await {
        if chunk.finish_reason.as_deref() == Some("tool_calls") {
            assert_eq!(chunk.tool_calls.len(), 1);
            let tc = &chunk.tool_calls[0];
            assert_eq!(tc.id, "call_1");
            assert_eq!(tc.function.as_ref().unwrap().name, "get_weather");
            assert_eq!(
                tc.function.as_ref().unwrap().arguments,
                "{\"loc\":\"SF\"}"
            );
            got_tool_call_chunk = true;
        }
    }
    assert!(got_tool_call_chunk);
}

#[tokio::test]
async fn test_chat_stream_multiple_tool_calls_with_index() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"tool_a\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"tool_b\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("run two tools")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    while let Some(Ok(chunk)) = rx.recv().await {
        if chunk.finish_reason.as_deref() == Some("tool_calls") {
            assert_eq!(chunk.tool_calls.len(), 2);
            let names: Vec<&str> = chunk
                .tool_calls
                .iter()
                .map(|tc| tc.function.as_ref().unwrap().name.as_str())
                .collect();
            assert!(names.contains(&"tool_a"));
            assert!(names.contains(&"tool_b"));
            return;
        }
    }
    panic!("never received tool_calls finish chunk");
}

#[tokio::test]
async fn test_chat_stream_http_error_returns_error_chunk() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let result = rx.recv().await.expect("should get a message");
    match result {
        Err(FailoverError::RateLimit { .. }) => {}
        other => panic!("Expected RateLimit, got {:?}", other),
    }
}

#[tokio::test]
async fn test_chat_stream_skips_malformed_sse_lines() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let sse_body = concat!(
        "data: not json\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut received = String::new();
    while let Some(Ok(chunk)) = rx.recv().await {
        received.push_str(&chunk.delta);
    }
    assert_eq!(received, "ok");
}

#[tokio::test]
async fn test_chat_stream_skips_empty_delta_chunks() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    // Chunks with empty delta, no finish, no tool calls should not emit
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut count = 0;
    while let Some(Ok(_)) = rx.recv().await {
        count += 1;
    }
    // Should be exactly 2: content chunk + [DONE] stop chunk
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_chat_stream_ignores_non_data_lines() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let sse_body = concat!(
        ": comment line\n\n",
        "event: ping\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
        "data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "gpt-4",
        &ChatOptions::default(),
    );

    let mut content = String::new();
    while let Some(Ok(chunk)) = rx.recv().await {
        content.push_str(&chunk.delta);
    }
    assert_eq!(content, "x");
}

#[tokio::test]
async fn test_chat_stream_empty_model_uses_default() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n"),
        )
        .mount(&server)
        .await;

    let mut rx = provider.chat_stream(
        &[user_message("hi")],
        &[],
        "",
        &ChatOptions::default(),
    );

    // Should produce a single stop chunk
    let chunk = rx.recv().await.expect("should get a chunk").expect("should be ok");
    assert_eq!(chunk.finish_reason.as_deref(), Some("stop"));
}

// ---------------------------------------------------------------------------
// Additional normalize_model edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_normalize_model_empty_string() {
    assert_eq!(HttpProvider::normalize_model(""), "");
}

#[test]
fn test_normalize_model_only_slash() {
    // Just a slash should leave empty string after the slash
    assert_eq!(HttpProvider::normalize_model("/"), "");
}

#[test]
fn test_normalize_model_multiple_slashes() {
    // Only the first slash splits off prefix
    assert_eq!(HttpProvider::normalize_model("a/b/c"), "b/c");
}

#[test]
fn test_normalize_model_just_whitespace() {
    assert_eq!(HttpProvider::normalize_model("    "), "");
}

#[test]
fn test_normalize_model_preserves_dashes() {
    assert_eq!(
        HttpProvider::normalize_model("deepseek-r1-distill-qwen"),
        "deepseek-r1-distill-qwen"
    );
}

// ---------------------------------------------------------------------------
// Request body construction edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_build_request_body_no_temperature_for_o_series_only() {
    let provider = HttpProvider::new(basic_config("https://api.test/v1".to_string()));

    // Only o1- and o3- prefixes suppress temperature; gpt-5 does NOT.
    let body = provider.build_request_body(
        &[],
        &[],
        "gpt-5",
        &ChatOptions {
            temperature: Some(0.7),
            ..Default::default()
        },
    );
    assert_eq!(body["temperature"], 0.7);

    let body2 = provider.build_request_body(
        &[],
        &[],
        "o1-preview",
        &ChatOptions {
            temperature: Some(0.7),
            ..Default::default()
        },
    );
    assert!(body2.get("temperature").is_none());
}

#[test]
fn test_build_request_body_sets_tool_choice_auto() {
    let provider = HttpProvider::new(basic_config("https://api.test/v1".to_string()));
    let tool = ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "x".to_string(),
            description: "y".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    };

    let body = provider.build_request_body(&[], &[tool], "gpt-4", &ChatOptions::default());
    assert_eq!(body["tool_choice"], "auto");
}

#[test]
fn test_build_request_body_no_tool_choice_without_tools() {
    let provider = HttpProvider::new(basic_config("https://api.test/v1".to_string()));

    let body = provider.build_request_body(&[], &[], "gpt-4", &ChatOptions::default());
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn test_build_request_body_includes_messages() {
    let provider = HttpProvider::new(basic_config("https://api.test/v1".to_string()));
    let messages = vec![
        user_message("hello"),
        Message {
            role: "assistant".to_string(),
            content: "hi there".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
    ];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["content"], "hello");
    assert_eq!(arr[1]["content"], "hi there");
}

#[test]
fn test_default_timeout_constant() {
    // default_timeout() is used via serde when timeout_secs is absent
    let json = r#"{"name":"x","base_url":"u","api_key":"k"}"#;
    let config: HttpProviderConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.timeout_secs, 120);
}

#[test]
fn test_http_provider_config_timeout_default_when_missing() {
    let json = r#"{"name":"x","base_url":"u","api_key":"k","default_model":"m"}"#;
    let config: HttpProviderConfig = serde_json::from_str(json).unwrap();
    // timeout_secs missing -> serde default applies
    assert_eq!(config.timeout_secs, 120);
    assert_eq!(config.default_model, "m");
    // headers missing -> default empty map
    assert!(config.headers.is_empty());
    // proxy missing -> default None
    assert!(config.proxy.is_none());
    // preserve_prefix missing -> default false
    assert!(!config.preserve_prefix);
}

// ---------------------------------------------------------------------------
// Config deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_http_provider_config_deserialize_with_proxy() {
    let json = r#"{
        "name": "p",
        "base_url": "http://x",
        "api_key": "k",
        "proxy": "http://proxy:8080",
        "preserve_prefix": true
    }"#;
    let config: HttpProviderConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.proxy.as_deref(), Some("http://proxy:8080"));
    assert!(config.preserve_prefix);
}

#[test]
fn test_http_provider_config_deserialize_with_headers() {
    let json = r#"{
        "name": "p",
        "base_url": "http://x",
        "api_key": "k",
        "headers": {"X-A": "1", "X-B": "2"}
    }"#;
    let config: HttpProviderConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.headers.len(), 2);
    assert_eq!(config.headers.get("X-A").unwrap(), "1");
}

#[test]
fn test_http_provider_config_proxy_defaults_none() {
    let json = r#"{"name":"x","base_url":"u","api_key":"k"}"#;
    let config: HttpProviderConfig = serde_json::from_str(json).unwrap();
    assert!(config.proxy.is_none());
    assert!(!config.preserve_prefix);
}

#[test]
fn test_http_provider_config_headers_default_empty() {
    let json = r#"{"name":"x","base_url":"u","api_key":"k"}"#;
    let config: HttpProviderConfig = serde_json::from_str(json).unwrap();
    assert!(config.headers.is_empty());
}

// ---------------------------------------------------------------------------
// StreamChunk serialization/deserialization with serde attrs
// ---------------------------------------------------------------------------

#[test]
fn test_stream_chunk_skip_serializing_none() {
    let chunk = StreamChunk {
        delta: "x".to_string(),
        tool_calls: vec![],
        finish_reason: None,
        usage: None,
        reasoning_content: None,
    };
    let json = serde_json::to_string(&chunk).unwrap();
    // None fields should be skipped
    assert!(!json.contains("finish_reason"));
    assert!(!json.contains("usage"));
    assert!(!json.contains("reasoning_content"));
}

#[test]
fn test_stream_chunk_full_roundtrip() {
    let original = StreamChunk {
        delta: "delta text".to_string(),
        tool_calls: vec![ToolCall {
            id: "id1".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "fn".to_string(),
                arguments: "{}".to_string(),
            }),
            name: None,
            arguments: None,
        }],
        finish_reason: Some("length".to_string()),
        usage: Some(UsageInfo {
            prompt_tokens: 1,
            completion_tokens: 2,
            total_tokens: 3,
            cached_tokens: Some(4),
            cache_creation_tokens: None,
            cache_read_tokens: Some(4),
        }),
        reasoning_content: Some("thinking".to_string()),
    };

    let json = serde_json::to_string(&original).unwrap();
    let deserialized: StreamChunk = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.delta, original.delta);
    assert_eq!(deserialized.tool_calls.len(), 1);
    assert_eq!(deserialized.finish_reason, original.finish_reason);
    assert_eq!(
        deserialized.usage.as_ref().unwrap().total_tokens,
        3
    );
    assert_eq!(deserialized.reasoning_content, original.reasoning_content);
}

// ---------------------------------------------------------------------------
// Tool-call repair integration tests (Phase 1)
// Mock HTTP server returns DSML/JSON-text/XML in content, no tool_calls →
// http_provider.chat() should repair → LLMResponse.tool_calls populated.
// ---------------------------------------------------------------------------

fn tool_def(name: &str) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: name.to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
        },
    }
}

#[tokio::test]
async fn test_repair_dsml_in_chat_response() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    // Server returns DSML in content, NO tool_calls structure, finish_reason=stop.
    let dsml = format!(
        "<{d}tool_calls>\n<{d}invoke name=\"read_file\">\n\
         <{d}parameter name=\"path\" string=\"true\">/tmp/test.rs</{d}parameter>\n\
         </{d}invoke>\n</{d}tool_calls>",
        d = "\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}"
    );
    let response_body = serde_json::json!({
        "choices": [{
            "message": { "role": "assistant", "content": dsml },
            "finish_reason": "stop"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("read the file")], &[tool_def("read_file")], "deepseek-v4-flash", &ChatOptions::default())
        .await
        .expect("chat should succeed");

    // Repair should have recovered the tool call from DSML content.
    assert_eq!(result.finish_reason, "tool_calls");
    assert_eq!(result.tool_calls.len(), 1, "should have 1 repaired tool call");
    assert_eq!(result.tool_calls[0].function.as_ref().unwrap().name, "read_file");
    let args: serde_json::Value = serde_json::from_str(
        &result.tool_calls[0].function.as_ref().unwrap().arguments
    ).unwrap();
    assert_eq!(args["path"], "/tmp/test.rs");
}

#[tokio::test]
async fn test_repair_json_text_in_chat_response() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I'll use the tool:\n```json\n[{\"name\": \"grep\", \"arguments\": {\"pattern\": \"TODO\"}}]\n```\ndone"
            },
            "finish_reason": "stop"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("search")], &[tool_def("grep")], "deepseek-v4-flash", &ChatOptions::default())
        .await
        .expect("chat should succeed");

    assert_eq!(result.finish_reason, "tool_calls");
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].function.as_ref().unwrap().name, "grep");
    let args: serde_json::Value = serde_json::from_str(
        &result.tool_calls[0].function.as_ref().unwrap().arguments
    ).unwrap();
    assert_eq!(args["pattern"], "TODO");
}

#[tokio::test]
async fn test_no_repair_when_standard_tool_calls_present() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    // Standard OpenAI tool_calls → repair should NOT fire (tool_calls already populated).
    let response_body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Let me help",
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\": \"/real.rs\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("read")], &[tool_def("read_file")], "deepseek-v4-flash", &ChatOptions::default())
        .await
        .unwrap();

    // Standard tool_calls preserved as-is (not repaired).
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].id, "call_abc");
    assert_eq!(result.tool_calls[0].function.as_ref().unwrap().name, "read_file");
}

#[tokio::test]
async fn test_no_repair_when_content_is_plain_text() {
    let server = MockServer::start().await;
    let provider = HttpProvider::new(basic_config(server.uri()));

    let response_body = serde_json::json!({
        "choices": [{
            "message": { "role": "assistant", "content": "Hello! I can help with that." },
            "finish_reason": "stop"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&server)
        .await;

    let result = provider
        .chat(&[user_message("hi")], &[tool_def("test")], "gpt-4", &ChatOptions::default())
        .await
        .unwrap();

    // Plain text → no tool calls, no repair.
    assert!(result.tool_calls.is_empty());
    assert_eq!(result.finish_reason, "stop");
}

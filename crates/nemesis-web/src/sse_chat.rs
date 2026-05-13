//! SSE chat streaming endpoint.
//!
//! Provides `/api/chat/stream` — a Server-Sent Events endpoint that streams
//! LLM response chunks directly to HTTP clients. This is useful for:
//! - Web UI chat with real-time token streaming
//! - API consumers that prefer SSE over WebSocket
//! - Future long-running data fetching from nemesisbot
//!
//! Unlike the WebSocket channel (which goes through the full AgentLoop with
//! tools, memory, etc.), this endpoint does a direct LLM call and streams
//! the response. For full agent capabilities, use the WebSocket channel.

use crate::api_handlers::AppState;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use nemesis_providers::types::{ChatOptions, Message};
use std::convert::Infallible;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for `/api/chat/stream`.
#[derive(Debug, serde::Deserialize)]
pub struct ChatStreamRequest {
    /// Conversation messages (role + content).
    pub messages: Vec<MessageEntry>,
    /// Model name (optional — uses provider default if empty).
    #[serde(default)]
    pub model: String,
    /// Temperature (optional).
    pub temperature: Option<f64>,
    /// Max tokens (optional).
    pub max_tokens: Option<i64>,
}

/// A single message in the chat stream request.
#[derive(Debug, serde::Deserialize)]
pub struct MessageEntry {
    pub role: String,
    pub content: String,
}

/// A chunk emitted as an SSE event.
#[derive(Debug, serde::Serialize)]
pub struct ChatStreamEvent {
    /// Incremental text content.
    pub delta: String,
    /// Finish reason (present only on the final chunk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Token usage (present only on the final chunk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Token usage info in SSE events.
#[derive(Debug, serde::Serialize)]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /api/chat/stream` — SSE streaming chat endpoint.
///
/// Accepts a JSON body with messages and streams LLM response chunks
/// as SSE events. The stream ends with a `[DONE]` event.
pub async fn handle_chat_stream(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(req): Json<ChatStreamRequest>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let provider = state.streaming_provider.clone();
    let model = if req.model.is_empty() {
        "".to_string()
    } else {
        req.model
    };

    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            role: m.role,
            content: m.content,
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        })
        .collect();

    let options = ChatOptions {
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        ..Default::default()
    };

    let stream = async_stream::stream! {
        let Some(provider) = provider else {
            yield Ok(SseEvent::default()
                .event("error")
                .data(r#"{"error":"No streaming provider configured"}"#));
            return;
        };

        let mut rx = provider.chat_stream(&messages, &[], &model, &options);

        while let Some(result) = rx.recv().await {
            match result {
                Ok(chunk) => {
                    let is_done = chunk.finish_reason.is_some();

                    let event = ChatStreamEvent {
                        delta: chunk.delta,
                        finish_reason: chunk.finish_reason,
                        usage: chunk.usage.map(|u| Usage {
                            prompt_tokens: u.prompt_tokens,
                            completion_tokens: u.completion_tokens,
                            total_tokens: u.total_tokens,
                        }),
                    };

                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok(SseEvent::default().event("chunk").data(data));

                    if is_done {
                        yield Ok(SseEvent::default().event("done").data("[DONE]"));
                        return;
                    }
                }
                Err(e) => {
                    let error_json = serde_json::json!({"error": e.to_string()});
                    yield Ok(SseEvent::default()
                        .event("error")
                        .data(error_json.to_string()));
                    return;
                }
            }
        }

        // Stream ended without [DONE] — send it anyway.
        yield Ok(SseEvent::default().event("done").data("[DONE]"));
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_stream_request_deserialize() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "model": "gpt-4",
            "temperature": 0.7
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.model, "gpt-4");
        assert_eq!(req.temperature, Some(0.7));
    }

    #[test]
    fn test_chat_stream_request_minimal() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": "Hi"}
            ]
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert!(req.model.is_empty());
        assert!(req.temperature.is_none());
    }

    #[test]
    fn test_chat_stream_request_with_max_tokens() {
        let json = r#"{
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ],
            "model": "test-1.0",
            "max_tokens": 100
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.max_tokens, Some(100));
    }

    #[test]
    fn test_chat_stream_event_serialize() {
        let event = ChatStreamEvent {
            delta: "Hello ".to_string(),
            finish_reason: None,
            usage: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Hello "));
        assert!(!json.contains("finish_reason"));
    }

    #[test]
    fn test_chat_stream_event_done() {
        let event = ChatStreamEvent {
            delta: String::new(),
            finish_reason: Some("stop".to_string()),
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            }),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("stop"));
        assert!(json.contains("30"));
    }

    #[test]
    fn test_message_entry_deserialize() {
        let json = r#"{"role": "assistant", "content": "world"}"#;
        let entry: MessageEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.role, "assistant");
        assert_eq!(entry.content, "world");
    }
}

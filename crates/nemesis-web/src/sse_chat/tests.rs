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

// ============================================================
// Additional coverage tests for SSE chat types
// ============================================================

#[test]
fn test_chat_stream_request_empty_messages() {
    let json = r#"{"messages": []}"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert!(req.messages.is_empty());
}

#[test]
fn test_chat_stream_request_multiple_messages() {
    let json = r#"{
        "messages": [
            {"role": "system", "content": "You are helpful"},
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi there"},
            {"role": "user", "content": "How are you?"}
        ]
    }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.messages.len(), 4);
    assert_eq!(req.messages[0].role, "system");
    assert_eq!(req.messages[3].content, "How are you?");
}

#[test]
fn test_chat_stream_request_all_fields() {
    let json = r#"{
        "messages": [{"role": "user", "content": "test"}],
        "model": "gpt-4o",
        "temperature": 0.5,
        "max_tokens": 2048
    }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.model, "gpt-4o");
    assert_eq!(req.temperature, Some(0.5));
    assert_eq!(req.max_tokens, Some(2048));
}

#[test]
fn test_chat_stream_event_delta_only() {
    let event = ChatStreamEvent {
        delta: "Hello world".to_string(),
        finish_reason: None,
        usage: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("Hello world"));
    // finish_reason and usage should not appear
    assert!(!json.contains("finish_reason"));
    assert!(!json.contains("usage"));
}

#[test]
fn test_chat_stream_event_with_usage() {
    let event = ChatStreamEvent {
        delta: "".to_string(),
        finish_reason: Some("stop".to_string()),
        usage: Some(Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        }),
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["finish_reason"], "stop");
    assert_eq!(parsed["usage"]["prompt_tokens"], 100);
    assert_eq!(parsed["usage"]["completion_tokens"], 50);
    assert_eq!(parsed["usage"]["total_tokens"], 150);
}

#[test]
fn test_usage_serialization() {
    let usage = Usage {
        prompt_tokens: 10,
        completion_tokens: 20,
        total_tokens: 30,
    };
    let json = serde_json::to_string(&usage).unwrap();
    let parsed: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.prompt_tokens, 10);
    assert_eq!(parsed.completion_tokens, 20);
    assert_eq!(parsed.total_tokens, 30);
}

#[test]
fn test_usage_zero_tokens() {
    let usage = Usage {
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
    };
    let json = serde_json::to_string(&usage).unwrap();
    assert!(json.contains("0"));
}

#[test]
fn test_usage_large_tokens() {
    let usage = Usage {
        prompt_tokens: i64::MAX,
        completion_tokens: i64::MAX,
        total_tokens: i64::MAX,
    };
    let json = serde_json::to_string(&usage).unwrap();
    let parsed: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.prompt_tokens, i64::MAX);
}

#[test]
fn test_chat_stream_request_deserialize_negative_temperature() {
    let json = r#"{
        "messages": [{"role": "user", "content": "test"}],
        "temperature": -0.5
    }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.temperature, Some(-0.5));
}

#[test]
fn test_chat_stream_request_deserialize_negative_max_tokens() {
    let json = r#"{
        "messages": [{"role": "user", "content": "test"}],
        "max_tokens": -100
    }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.max_tokens, Some(-100));
}

#[test]
fn test_message_entry_role_types() {
    for role in &["user", "assistant", "system", "tool"] {
        let json = format!(r#"{{"role": "{}", "content": "test"}}"#, role);
        let entry: MessageEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry.role, *role);
    }
}

#[test]
fn test_chat_stream_event_with_finish_reason_length() {
    let event = ChatStreamEvent {
        delta: "".to_string(),
        finish_reason: Some("length".to_string()),
        usage: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("length"));
    // usage should not appear since it's None
    assert!(!json.contains("usage"));
}

#[test]
fn test_chat_stream_event_with_finish_reason_tool_calls() {
    let event = ChatStreamEvent {
        delta: "".to_string(),
        finish_reason: Some("tool_calls".to_string()),
        usage: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("tool_calls"));
}

#[test]
fn test_chat_stream_request_invalid_json() {
    let result = serde_json::from_str::<ChatStreamRequest>("not json");
    assert!(result.is_err());
}

#[test]
fn test_chat_stream_request_missing_messages() {
    let json = r#"{"model": "gpt-4"}"#;
    let result = serde_json::from_str::<ChatStreamRequest>(json);
    assert!(result.is_err());
}

#[test]
fn test_message_entry_empty_content() {
    let json = r#"{"role": "user", "content": ""}"#;
    let entry: MessageEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.content, "");
}

#[test]
fn test_message_entry_unicode_content() {
    let json = r#"{"role": "user", "content": "Hello \u4e16\u754c"}"#;
    let entry: MessageEntry = serde_json::from_str(json).unwrap();
    assert!(entry.content.contains("\u{4e16}"));
}

#[test]
fn test_chat_stream_request_model_default_empty() {
    let json = r#"{"messages": []}"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.model, "");
}

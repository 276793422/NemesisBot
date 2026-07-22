use super::*;

#[test]
fn test_valid_request_no_provider() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello"}],
        "model": "test-model"
    });

    let result = handler.handle(payload);
    assert!(result.success);
    assert_eq!(result.response["model"], "test-model");
    assert_eq!(result.response["validated"], true);
}

#[test]
fn test_missing_messages() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({"model": "test"});

    let result = handler.handle(payload);
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_empty_messages() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({"messages": []});

    let result = handler.handle(payload);
    assert!(!result.success);
}

#[test]
fn test_validate_valid() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
        ],
    });
    assert!(handler.validate(&payload).is_ok());
}

#[test]
fn test_validate_missing_role() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": [{"content": "hello"}],
    });
    let result = handler.validate(&payload);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing 'role'"));
}

#[test]
fn test_validate_missing_content() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": [{"role": "user"}],
    });
    let result = handler.validate(&payload);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing 'content'"));
}

// -- Mock provider for testing --

struct MockLlmProvider {
    response: String,
    should_fail: bool,
}

impl LlmProvider for MockLlmProvider {
    fn chat_completion(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        _options: &serde_json::Value,
    ) -> Result<String, String> {
        if self.should_fail {
            return Err("LLM provider error".into());
        }
        Ok(format!(
            "Response from {} to {} messages: {}",
            model,
            messages.len(),
            self.response
        ))
    }
}

#[test]
fn test_with_provider_success() {
    let provider = Arc::new(MockLlmProvider {
        response: "Hello!".into(),
        should_fail: false,
    });
    let handler = LlmProxyHandler::with_provider("node-a".into(), provider);

    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello"}],
        "model": "gpt-4",
    });

    let result = handler.handle(payload);
    assert!(result.success);
    let content = result.response["content"].as_str().unwrap();
    assert!(content.contains("Response from gpt-4"));
    assert!(content.contains("Hello!"));
}

#[test]
fn test_with_provider_failure() {
    let provider = Arc::new(MockLlmProvider {
        response: String::new(),
        should_fail: true,
    });
    let handler = LlmProxyHandler::with_provider("node-a".into(), provider);

    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello"}],
    });

    let result = handler.handle(payload);
    assert!(!result.success);
    assert!(result.error.unwrap().contains("LLM error"));
}

#[test]
fn test_set_provider() {
    let mut handler = LlmProxyHandler::new("node-a".into());
    handler.set_provider(Arc::new(MockLlmProvider {
        response: "test".into(),
        should_fail: false,
    }));

    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello"}],
    });

    let result = handler.handle(payload);
    assert!(result.success);
}

#[test]
fn test_set_default_model() {
    let mut handler = LlmProxyHandler::new("node-a".into());
    handler.set_default_model("claude-3");

    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello"}],
    });

    let result = handler.handle(payload);
    assert_eq!(result.response["model"], "claude-3");
}

#[test]
fn test_options_forwarded() {
    let provider = Arc::new(MockLlmProvider {
        response: "response".into(),
        should_fail: false,
    });
    let handler = LlmProxyHandler::with_provider("node-a".into(), provider);

    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello"}],
        "options": {"temperature": 0.7, "max_tokens": 100},
    });

    let result = handler.handle(payload);
    assert!(result.success);
}

// -- Additional tests: handle with non-array messages, validate missing fields --

#[test]
fn test_handle_messages_not_array() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": "not an array"
    });

    let result = handler.handle(payload);
    assert!(!result.success);
    let err = result.error.unwrap();
    assert!(
        err.contains("messages must be a non-empty array"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_handle_messages_is_number() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": 42
    });

    let result = handler.handle(payload);
    assert!(!result.success);
    let err = result.error.unwrap();
    assert!(
        err.contains("messages must be a non-empty array"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_validate_messages_missing_field() {
    let handler = LlmProxyHandler::new("node-a".into());

    // Missing "role"
    let payload_no_role = serde_json::json!({
        "messages": [{"content": "hello"}]
    });
    let err = handler.validate(&payload_no_role).unwrap_err();
    assert!(err.contains("missing 'role'"), "unexpected: {}", err);

    // Missing "content"
    let payload_no_content = serde_json::json!({
        "messages": [{"role": "user"}]
    });
    let err = handler.validate(&payload_no_content).unwrap_err();
    assert!(err.contains("missing 'content'"), "unexpected: {}", err);

    // Missing both "role" and "content" - should fail on role first
    let payload_no_both = serde_json::json!({
        "messages": [{"something": "else"}]
    });
    let err = handler.validate(&payload_no_both).unwrap_err();
    assert!(err.contains("missing 'role'"), "unexpected: {}", err);
}

#[test]
fn test_validate_messages_second_message_missing_role() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": [
            {"role": "user", "content": "hello"},
            {"content": "no role here"}
        ]
    });
    let err = handler.validate(&payload).unwrap_err();
    assert!(
        err.contains("message 1 missing 'role'"),
        "unexpected: {}",
        err
    );
}

#[test]
fn test_validate_messages_second_message_missing_content() {
    let handler = LlmProxyHandler::new("node-a".into());
    let payload = serde_json::json!({
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant"}
        ]
    });
    let err = handler.validate(&payload).unwrap_err();
    assert!(
        err.contains("message 1 missing 'content'"),
        "unexpected: {}",
        err
    );
}

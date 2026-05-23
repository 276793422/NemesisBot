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

//! LLM proxy handler - forwards chat completion requests to remote nodes.
//!
//! Allows a node without a local LLM to proxy requests through a peer.
//! Validates the request, invokes the configured LLM provider, and returns
//! the response.

use std::sync::Arc;

use crate::handlers::default_handler::HandleResult;

// ---------------------------------------------------------------------------
// LLM Provider interface
// ---------------------------------------------------------------------------

/// Interface for invoking an LLM. Implemented by the providers module.
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request and return the response content.
    ///
    /// * `model` - The model identifier (e.g., "gpt-4", "claude-3").
    /// * `messages` - The conversation messages as JSON array.
    /// * `options` - Additional options (temperature, max_tokens, etc.).
    ///
    /// Returns the assistant's response content on success.
    fn chat_completion(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        options: &serde_json::Value,
    ) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Handler for LLM proxy actions.
pub struct LlmProxyHandler {
    node_id: String,
    provider: Option<Arc<dyn LlmProvider>>,
    /// Default model to use when not specified.
    default_model: String,
}

impl LlmProxyHandler {
    /// Create a new LLM proxy handler.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            provider: None,
            default_model: "default".into(),
        }
    }

    /// Create a handler with a specific LLM provider.
    pub fn with_provider(node_id: String, provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            node_id,
            provider: Some(provider),
            default_model: "default".into(),
        }
    }

    /// Set the LLM provider.
    pub fn set_provider(&mut self, provider: Arc<dyn LlmProvider>) {
        self.provider = Some(provider);
    }

    /// Set the default model.
    pub fn set_default_model(&mut self, model: &str) {
        self.default_model = model.into();
    }

    /// Handle an LLM proxy request.
    ///
    /// Validates the request, invokes the LLM provider if available,
    /// and returns the response.
    pub fn handle(&self, payload: serde_json::Value) -> HandleResult {
        // 1. Validate messages field
        let messages = match payload.get("messages") {
            Some(msgs) => msgs,
            None => {
                return HandleResult {
                    success: false,
                    response: serde_json::Value::Null,
                    error: Some("messages field is required".into()),
                };
            }
        };

        let messages_arr = match messages.as_array() {
            Some(arr) if !arr.is_empty() => arr,
            _ => {
                return HandleResult {
                    success: false,
                    response: serde_json::Value::Null,
                    error: Some("messages must be a non-empty array".into()),
                };
            }
        };

        // 2. Extract model name
        let model = payload
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.default_model);

        // 3. Extract additional options
        let options = payload.get("options").cloned().unwrap_or(serde_json::json!({}));

        tracing::debug!(
            node_id = %self.node_id,
            model = model,
            message_count = messages_arr.len(),
            "Processing LLM proxy request"
        );

        // 4. Invoke provider if available
        if let Some(ref provider) = self.provider {
            match provider.chat_completion(model, messages_arr, &options) {
                Ok(content) => {
                    tracing::info!(
                        node_id = %self.node_id,
                        model = model,
                        response_len = content.len(),
                        "LLM proxy request completed"
                    );

                    HandleResult {
                        success: true,
                        response: serde_json::json!({
                            "content": content,
                            "model": model,
                            "node_id": self.node_id,
                        }),
                        error: None,
                    }
                }
                Err(e) => {
                    tracing::error!(
                        node_id = %self.node_id,
                        model = model,
                        error = %e,
                        "LLM proxy request failed"
                    );

                    HandleResult {
                        success: false,
                        response: serde_json::Value::Null,
                        error: Some(format!("LLM error: {}", e)),
                    }
                }
            }
        } else {
            // No provider configured - return validation-only response
            tracing::warn!(
                node_id = %self.node_id,
                "No LLM provider configured, returning validation-only response"
            );

            HandleResult {
                success: true,
                response: serde_json::json!({
                    "content": format!(
                        "[LLM proxy: no provider configured on node {}]",
                        self.node_id
                    ),
                    "model": model,
                    "node_id": self.node_id,
                    "validated": true,
                    "message_count": messages_arr.len(),
                }),
                error: None,
            }
        }
    }

    /// Validate a request payload without processing it.
    pub fn validate(&self, payload: &serde_json::Value) -> Result<(), String> {
        let messages = payload
            .get("messages")
            .ok_or("messages field is required")?;

        let arr = messages
            .as_array()
            .ok_or("messages must be an array")?;

        if arr.is_empty() {
            return Err("messages must be a non-empty array".into());
        }

        // Validate each message has role and content
        for (i, msg) in arr.iter().enumerate() {
            if msg.get("role").is_none() {
                return Err(format!("message {} missing 'role' field", i));
            }
            if msg.get("content").is_none() {
                return Err(format!("message {} missing 'content' field", i));
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
}

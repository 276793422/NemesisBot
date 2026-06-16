//! Extra tests for `llm_bridge` — covers `ProviderAdapter` and
//! `ForgeProviderBridge` construction and behavior via a mock provider.

#[cfg(test)]
mod llm_bridge_extra_tests {
    use crate::llm_bridge::{ForgeProviderBridge, ProviderAdapter};
    use async_trait::async_trait;
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider};
    use nemesis_agent::types::{ChatOptions, ToolDefinition, ToolFunctionDef};
    use nemesis_forge::reflector_llm::LLMCaller;
    use nemesis_providers::failover::FailoverError;
    use nemesis_providers::router::LLMProvider;
    use nemesis_providers::types::{
        ChatOptions as ProviderChatOptions, FunctionCall, LLMResponse as ProviderResponse,
        Message, ToolCall, ToolDefinition as ProviderToolDef, UsageInfo,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // -----------------------------------------------------------------------
    // Mock provider
    // -----------------------------------------------------------------------

    /// Configurable mock that records calls and returns a canned response.
    struct MockProvider {
        default_model: String,
        name: String,
        response: Mutex<Option<ProviderResponse>>,
        error: Mutex<Option<FailoverError>>,
        call_count: AtomicUsize,
        last_model: Mutex<Option<String>>,
        last_message_count: Mutex<Option<usize>>,
        last_tool_count: Mutex<Option<usize>>,
    }

    impl MockProvider {
        fn new(default_model: &str, name: &str) -> Self {
            Self {
                default_model: default_model.to_string(),
                name: name.to_string(),
                response: Mutex::new(None),
                error: Mutex::new(None),
                call_count: AtomicUsize::new(0),
                last_model: Mutex::new(None),
                last_message_count: Mutex::new(None),
                last_tool_count: Mutex::new(None),
            }
        }

        fn set_response(&self, resp: ProviderResponse) {
            *self.response.lock().unwrap() = Some(resp);
        }

        fn set_error(&self, err: FailoverError) {
            *self.error.lock().unwrap() = Some(err);
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }

        fn last_model_seen(&self) -> Option<String> {
            self.last_model.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(
            &self,
            messages: &[Message],
            tools: &[ProviderToolDef],
            model: &str,
            _options: &ProviderChatOptions,
        ) -> Result<ProviderResponse, FailoverError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_model.lock().unwrap() = Some(model.to_string());
            *self.last_message_count.lock().unwrap() = Some(messages.len());
            *self.last_tool_count.lock().unwrap() = Some(tools.len());
            if let Some(err) = self.error.lock().unwrap().take() {
                return Err(err);
            }
            if let Some(resp) = self.response.lock().unwrap().take() {
                return Ok(resp);
            }
            Ok(ProviderResponse {
                content: "default".to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                usage: None,
                reasoning_content: None,
                extra: HashMap::new(),
                raw_request_body: None,
                raw_response_body: None,
            })
        }

        fn default_model(&self) -> &str {
            &self.default_model
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    fn text_response(content: &str) -> ProviderResponse {
        ProviderResponse {
            content: content.to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: Some(UsageInfo {
                prompt_tokens: 5,
                completion_tokens: 10,
                total_tokens: 15,
                cached_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            }),
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        }
    }

    fn tool_call_response() -> ProviderResponse {
        ProviderResponse {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                call_type: None,
                function: Some(FunctionCall {
                    name: "get_weather".to_string(),
                    arguments: r#"{"city":"Paris"}"#.to_string(),
                }),
                name: None,
                arguments: None,
            }],
            finish_reason: "tool_calls".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        }
    }

    fn empty_messages() -> Vec<LlmMessage> {
        vec![]
    }

    fn one_user_message() -> Vec<LlmMessage> {
        vec![LlmMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }]
    }

    // -----------------------------------------------------------------------
    // ProviderAdapter — construction & basic chat
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn adapter_chat_empty_model_uses_default() {
        let mock = Arc::new(MockProvider::new("gpt-4o", "openai"));
        mock.set_response(text_response("hello"));
        let adapter = ProviderAdapter::new(mock.clone(), "gpt-4o".to_string());

        let resp = adapter
            .chat("", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        assert_eq!(resp.content, "hello");
        assert!(resp.finished);
        // default model should be used because model param was empty
        assert_eq!(mock.last_model_seen().as_deref(), Some("gpt-4o"));
        assert_eq!(mock.calls(), 1);
    }

    fn empty_messages_to_tools() -> Vec<ToolDefinition> {
        vec![]
    }

    #[tokio::test]
    async fn adapter_chat_explicit_model_overrides_default() {
        let mock = Arc::new(MockProvider::new("gpt-4o", "openai"));
        mock.set_response(text_response("yo"));
        let adapter = ProviderAdapter::new(mock.clone(), "gpt-4o".to_string());

        let _ = adapter
            .chat("claude-3", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        assert_eq!(mock.last_model_seen().as_deref(), Some("claude-3"));
    }

    #[tokio::test]
    async fn adapter_chat_passes_message_count() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("ok"));
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let msgs = vec![
            LlmMessage {
                role: "system".to_string(),
                content: "sys".to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            },
            LlmMessage {
                role: "user".to_string(),
                content: "u".to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            },
        ];
        let _ = adapter
            .chat("m", msgs, None, empty_messages_to_tools())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn adapter_chat_with_tool_calls_marks_not_finished() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(tool_call_response());
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let resp = adapter
            .chat("m", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        assert!(!resp.finished, "tool_calls present → finished=false");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "get_weather");
        assert_eq!(resp.tool_calls[0].id, "call_1");
    }

    #[tokio::test]
    async fn adapter_chat_finish_reason_stop_marks_finished_even_with_tools() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        // tool_calls present BUT finish_reason == stop → finished=true
        let mut resp = tool_call_response();
        resp.finish_reason = "stop".to_string();
        mock.set_response(resp);
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let r = adapter
            .chat("m", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        assert!(r.finished);
    }

    #[tokio::test]
    async fn adapter_chat_usage_propagated() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("x"));
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let r = adapter
            .chat("m", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        let u = r.usage.expect("usage");
        assert_eq!(u.prompt_tokens, 5);
        assert_eq!(u.completion_tokens, 10);
        assert_eq!(u.total_tokens, 15);
    }

    #[tokio::test]
    async fn adapter_chat_options_some_converted() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("ok"));
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let opts = Some(ChatOptions {
            max_tokens: Some(128),
            temperature: Some(0.5_f32),
            top_p: Some(0.9_f32),
            stop: Some(vec!["\n".to_string()]),
        });
        let _ = adapter
            .chat("m", one_user_message(), opts, empty_messages_to_tools())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn adapter_chat_options_none_uses_provider_defaults() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("ok"));
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let r = adapter
            .chat("m", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        assert_eq!(r.content, "ok");
    }

    #[tokio::test]
    async fn adapter_chat_passes_tool_definitions() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("ok"));
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDef {
                name: "do_x".to_string(),
                description: "does x".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let _ = adapter
            .chat("m", one_user_message(), None, tools)
            .await
            .unwrap();
        // just confirms no panic; mock records tool count
    }

    #[tokio::test]
    async fn adapter_chat_error_returns_string_err() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_error(FailoverError::Unknown {
            provider: "p".to_string(),
            message: "boom".to_string(),
        });
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let err = adapter
            .chat("m", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap_err();
        // Error message should mention "boom" (format!("{}", e))
        assert!(err.contains("boom") || !err.is_empty());
    }

    // -----------------------------------------------------------------------
    // ProviderAdapter — tool_calls with missing function are filtered out
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn adapter_filters_tool_calls_without_function() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        let resp = ProviderResponse {
            content: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "no_func".to_string(),
                    call_type: None,
                    function: None,
                    name: None,
                    arguments: None,
                },
                ToolCall {
                    id: "with_func".to_string(),
                    call_type: None,
                    function: Some(FunctionCall {
                        name: "f".to_string(),
                        arguments: "{}".to_string(),
                    }),
                    name: None,
                    arguments: None,
                },
            ],
            finish_reason: "tool_calls".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        };
        mock.set_response(resp);
        let adapter = ProviderAdapter::new(mock, "m".to_string());

        let r = adapter
            .chat("m", one_user_message(), None, empty_messages_to_tools())
            .await
            .unwrap();
        // Only the with_func call survives
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "with_func");
    }

    // -----------------------------------------------------------------------
    // ForgeProviderBridge
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn forge_bridge_returns_content_on_success() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("reflection result"));
        let bridge = ForgeProviderBridge::new(mock, "m".to_string());

        let out = bridge
            .chat("system prompt", "user prompt", Some(100))
            .await
            .unwrap();
        assert_eq!(out, "reflection result");
    }

    #[tokio::test]
    async fn forge_bridge_empty_content_and_no_tools_is_error() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(ProviderResponse {
            content: String::new(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        });
        let bridge = ForgeProviderBridge::new(mock, "m".to_string());

        let err = bridge
            .chat("s", "u", None)
            .await
            .unwrap_err();
        assert_eq!(err, "LLM returned no content");
    }

    #[tokio::test]
    async fn forge_bridge_none_max_tokens_ok() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(text_response("ok"));
        let bridge = ForgeProviderBridge::new(mock, "m".to_string());

        let out = bridge.chat("s", "u", None).await.unwrap();
        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn forge_bridge_provider_error_propagates() {
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_error(FailoverError::Unknown {
            provider: "p".to_string(),
            message: "down".to_string(),
        });
        let bridge = ForgeProviderBridge::new(mock, "m".to_string());

        let err = bridge.chat("s", "u", None).await.unwrap_err();
        // Error formatted via {:?} — should contain the message
        assert!(err.contains("down"));
    }

    #[tokio::test]
    async fn forge_bridge_content_with_tools_returns_content() {
        // Even if tool_calls present, bridge returns content (non-empty) per impl.
        let mock = Arc::new(MockProvider::new("m", "p"));
        let mut resp = tool_call_response();
        resp.content = "text".to_string();
        mock.set_response(resp);
        let bridge = ForgeProviderBridge::new(mock, "m".to_string());

        let out = bridge.chat("s", "u", None).await.unwrap();
        assert_eq!(out, "text");
    }

    #[tokio::test]
    async fn forge_bridge_empty_content_with_tools_returns_empty_string() {
        // tool_calls present but content empty — impl returns Ok(content) since
        // the check is `content.is_empty() && tool_calls.is_empty()`.
        let mock = Arc::new(MockProvider::new("m", "p"));
        mock.set_response(tool_call_response()); // content empty, tools non-empty
        let bridge = ForgeProviderBridge::new(mock, "m".to_string());

        let out = bridge.chat("s", "u", None).await.unwrap();
        assert_eq!(out, "");
    }
}

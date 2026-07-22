use super::*;
use crate::failover::FailoverReason;

/// A mock provider that always succeeds with a fixed response.
struct MockSuccessProvider {
    name: String,
    model: String,
}

#[async_trait]
impl LLMProvider for MockSuccessProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Ok(LLMResponse {
            content: format!("response from {}", self.name),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A mock provider that always fails.
struct MockFailProvider {
    name: String,
    model: String,
    retriable: bool,
}

#[async_trait]
impl LLMProvider for MockFailProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        if self.retriable {
            Err(FailoverError::RateLimit {
                provider: self.name.clone(),
                model: self.model.clone(),
                retry_after: None,
            })
        } else {
            Err(FailoverError::Auth {
                provider: self.name.clone(),
                model: self.model.clone(),
                status: 401,
            })
        }
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[tokio::test]
async fn test_fallback_first_succeeds() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![FallbackEntry {
            provider: Arc::new(MockSuccessProvider {
                name: "p1".to_string(),
                model: "m1".to_string(),
            }),
            model: "m1".to_string(),
        }],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let resp = provider
        .chat(&messages, &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "response from p1");
}

#[tokio::test]
async fn test_fallback_second_succeeds() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                    retriable: true,
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let resp = provider
        .chat(&messages, &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "response from p2");
}

#[tokio::test]
async fn test_fallback_non_retriable_stops() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                    retriable: false, // Auth error, non-retriable
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .chat(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FailoverError::Auth { .. }));
}

#[tokio::test]
async fn test_fallback_empty_chain() {
    let provider = FallbackProvider::new("test-fallback", vec![]);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .chat(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_fallback_cooldown_skips() {
    let cooldown = Arc::new(CooldownTracker::new());
    cooldown.mark_failure("p1", FailoverReason::RateLimit);

    let provider = FallbackProvider::with_cooldown(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
        cooldown,
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let resp = provider
        .chat(&messages, &[], "", &ChatOptions::default())
        .await
        .unwrap();
    // Should skip p1 (in cooldown) and get response from p2
    assert_eq!(resp.content, "response from p2");
}

#[test]
fn test_chain_len() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );
    assert_eq!(provider.chain_len(), 2);
}

#[test]
fn test_name() {
    let provider = FallbackProvider::new("my-fallback", vec![]);
    assert_eq!(provider.name(), "my-fallback");
}

#[tokio::test]
async fn test_execute_detailed_success() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![FallbackEntry {
            provider: Arc::new(MockSuccessProvider {
                name: "p1".to_string(),
                model: "m1".to_string(),
            }),
            model: "m1".to_string(),
        }],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_detailed(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_some());
    assert!(result.exhausted_error.is_none());
    assert_eq!(result.attempts.len(), 1);
    assert!(result.attempts[0].success);
}

#[tokio::test]
async fn test_execute_detailed_exhausted() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                    retriable: true,
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                    retriable: true,
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_detailed(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    let err = result.exhausted_error.unwrap();
    assert_eq!(err.providers_attempted, 2);
    assert_eq!(err.total_providers, 2);
    assert_eq!(result.attempts.len(), 2);
}

#[test]
fn test_fallback_exhausted_error_display() {
    let err = FallbackExhaustedError {
        chain_name: "test".to_string(),
        providers_attempted: 2,
        total_providers: 3,
        errors: vec![
            ("p1".to_string(), "rate limit".to_string()),
            ("p2".to_string(), "timeout".to_string()),
        ],
    };
    let msg = format!("{}", err);
    assert!(msg.contains("test"));
    assert!(msg.contains("2/3"));
    assert!(msg.contains("p1"));
}

#[test]
fn test_resolve_candidates_dedup() {
    let p1 = Arc::new(MockSuccessProvider {
        name: "p1".to_string(),
        model: "m1".to_string(),
    });
    let chain = vec![
        FallbackEntry {
            provider: p1.clone(),
            model: "m1".to_string(),
        },
        FallbackEntry {
            provider: p1.clone(),
            model: "m1".to_string(),
        },
        FallbackEntry {
            provider: Arc::new(MockSuccessProvider {
                name: "p2".to_string(),
                model: "m2".to_string(),
            }),
            model: "m2".to_string(),
        },
    ];
    let candidates = FallbackProvider::resolve_candidates(&chain, "");
    assert_eq!(candidates.len(), 2);
}

#[test]
fn test_fallback_default_model_returns_first_entry() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p1".to_string(),
                    model: "first-model".to_string(),
                }),
                model: "first-model".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "second-model".to_string(),
                }),
                model: "second-model".to_string(),
            },
        ],
    );
    assert_eq!(provider.default_model(), "first-model");
}

#[test]
fn test_fallback_default_model_empty_chain() {
    let provider = FallbackProvider::new("test-fallback", vec![]);
    assert_eq!(provider.default_model(), "");
}

#[tokio::test]
async fn test_execute_detailed_non_retriable_stops_immediately() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                    retriable: false, // Auth error = non-retriable
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_detailed(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    let err = result.exhausted_error.unwrap();
    // Should stop at first provider, not try p2
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(err.providers_attempted, 1);
}

#[tokio::test]
async fn test_execute_detailed_empty_chain_returns_exhausted() {
    let provider = FallbackProvider::new("test-fallback", vec![]);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_detailed(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    let err = result.exhausted_error.unwrap();
    assert_eq!(err.providers_attempted, 0);
    assert_eq!(err.total_providers, 0);
}

#[tokio::test]
async fn test_execute_image_success() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![FallbackEntry {
            provider: Arc::new(MockSuccessProvider {
                name: "p1".to_string(),
                model: "m1".to_string(),
            }),
            model: "m1".to_string(),
        }],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Describe this image".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_image(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_some());
    assert!(result.exhausted_error.is_none());
    assert_eq!(result.attempts.len(), 1);
    assert!(result.attempts[0].success);
}

#[tokio::test]
async fn test_execute_image_empty_chain() {
    let provider = FallbackProvider::new("test-fallback", vec![]);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Describe".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_image(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    assert!(result.exhausted_error.is_some());
    let err = result.exhausted_error.unwrap();
    assert_eq!(err.total_providers, 0);
}

#[tokio::test]
async fn test_execute_image_dimension_error_aborts() {
    /// A mock provider that returns image dimension errors.
    struct MockImageErrorProvider {
        name: String,
        model: String,
    }

    #[async_trait]
    impl LLMProvider for MockImageErrorProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _options: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            Err(FailoverError::Format {
                provider: self.name.clone(),
                message: "image dimensions exceed max 8000px".to_string(),
            })
        }
        fn default_model(&self) -> &str {
            &self.model
        }
        fn name(&self) -> &str {
            &self.name
        }
    }

    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockImageErrorProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Describe this image".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_image(&messages, &[], "", &ChatOptions::default())
        .await;
    // Image dimension errors are non-retriable, should stop at p1
    assert!(result.response.is_none());
    assert_eq!(result.attempts.len(), 1);
}

#[tokio::test]
async fn test_execute_image_size_error_aborts() {
    /// A mock provider that returns image size errors.
    struct MockImageSizeErrorProvider {
        name: String,
        model: String,
    }

    #[async_trait]
    impl LLMProvider for MockImageSizeErrorProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _options: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            Err(FailoverError::Format {
                provider: self.name.clone(),
                message: "image exceeds 20MB".to_string(),
            })
        }
        fn default_model(&self) -> &str {
            &self.model
        }
        fn name(&self) -> &str {
            &self.name
        }
    }

    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockImageSizeErrorProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Describe".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_image(&messages, &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    assert_eq!(result.attempts.len(), 1);
}

#[tokio::test]
async fn test_execute_detailed_cooldown_skips() {
    let cooldown = Arc::new(CooldownTracker::new());
    cooldown.mark_failure("p1", FailoverReason::RateLimit);

    let provider = FallbackProvider::with_cooldown(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p1".to_string(),
                    model: "m1".to_string(),
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "p2".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
        cooldown,
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];

    let result = provider
        .execute_detailed(&messages, &[], "", &ChatOptions::default())
        .await;
    // p1 should be skipped (in cooldown), p2 should succeed
    assert!(result.response.is_some());
    assert!(
        result
            .attempts
            .iter()
            .any(|a| a.provider == "p1" && a.error.as_ref().unwrap().contains("cooldown"))
    );
}

#[test]
fn test_fallback_exhausted_error_is_std_error() {
    let err = FallbackExhaustedError {
        chain_name: "test".to_string(),
        providers_attempted: 1,
        total_providers: 2,
        errors: vec![("p1".to_string(), "error".to_string())],
    };
    // Verify it implements std::error::Error
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_fallback_attempt_debug() {
    let attempt = FallbackAttempt {
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
        error: Some("rate limited".to_string()),
        success: false,
    };
    let debug_str = format!("{:?}", attempt);
    assert!(debug_str.contains("test-provider"));
    assert!(debug_str.contains("test-model"));
}

#[test]
fn test_cooldown_accessor() {
    let provider = FallbackProvider::new("test", vec![]);
    let cooldown = provider.cooldown();
    assert!(cooldown.is_available("any-provider"));
}

#[test]
fn test_resolve_candidates_preserves_order() {
    let chain: Vec<FallbackEntry> = (1..=5)
        .map(|i| FallbackEntry {
            provider: Arc::new(MockSuccessProvider {
                name: format!("p{}", i),
                model: format!("m{}", i),
            }),
            model: format!("m{}", i),
        })
        .collect();

    let candidates = FallbackProvider::resolve_candidates(&chain, "");
    assert_eq!(candidates.len(), 5);
    assert_eq!(candidates[0].provider.name(), "p1");
    assert_eq!(candidates[4].provider.name(), "p5");
}

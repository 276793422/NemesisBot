use super::*;
use crate::failover::FailoverReason;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// A mock provider that succeeds and counts its invocations.
struct MockCountingSuccessProvider {
    name: String,
    model: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LLMProvider for MockCountingSuccessProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
        content: "Hello".into(),
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
        content: "Hello".into(),
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

fn hello_messages() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: "Hello".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }]
}

/// BUG（cooldown 键碰撞）回归锁：生产装配下同 lane 链各级 provider.name()
/// 相同（factory 对 HttpCompat 恒发 "http-compat"，anthropic/codex lane
/// 同理）。冷却键若只按 lane 名，第一级 retriable 失败 mark_failure 后，
/// **同一请求内**第二级 is_available 即 false 被 skip——故障转移整链失效。
/// 修复后（键 = lane 名 + 本级生效模型名），第二级必须仍被尝试并接住。
#[tokio::test]
async fn test_fallback_same_provider_name_chain_still_rotates() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "http-compat".to_string(),
                    model: "m1".to_string(),
                    retriable: true,
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "http-compat".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let resp = provider
        .chat(&hello_messages(), &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "response from http-compat");
}

/// 同键隔离回归锁：第一级失败只冷却自己的（lane, model）键。请求 1 里
/// 第二级接住后，请求 2 第一级冷却中被 skip、但第二级**不受牵连**仍被
/// 尝试并成功——修复前两级共享 lane 名键，请求 2 全链 skip、整体返回
/// Unknown（不带真实根因）。
#[tokio::test]
async fn test_fallback_same_name_cooldown_isolated_per_level() {
    let second = Arc::new(MockCountingSuccessProvider {
        name: "http-compat".to_string(),
        model: "m2".to_string(),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let second_calls = Arc::clone(&second.calls);
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "http-compat".to_string(),
                    model: "m1".to_string(),
                    retriable: true,
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: second,
                model: "m2".to_string(),
            },
        ],
    );

    // 请求 1：第一级 retriable 失败 → 第二级接住（第 1 次调用）。
    let resp = provider
        .chat(&hello_messages(), &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "response from http-compat");
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);

    // 请求 2：第一级在冷却（键 http-compat:m1）被 skip；第二级
    // （键 http-compat:m2）不在冷却 → 仍被尝试（第 2 次调用）。
    let resp = provider
        .chat(&hello_messages(), &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "response from http-compat");
    assert_eq!(
        second_calls.load(Ordering::SeqCst),
        2,
        "第二级不得被第一级的失败冷却牵连"
    );
}

/// execute_detailed 同键路径回归锁：同 lane 链第一级 retriable 失败后，
/// 第二级在同一请求内仍被真实尝试（修复前 attempts 只有 1 条真实尝试，
/// 第二级是 "skipped: in cooldown" 占位）。
#[tokio::test]
async fn test_fallback_detailed_same_name_chain_rotates() {
    let provider = FallbackProvider::new(
        "test-fallback",
        vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider {
                    name: "http-compat".to_string(),
                    model: "m1".to_string(),
                    retriable: true,
                }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: "http-compat".to_string(),
                    model: "m2".to_string(),
                }),
                model: "m2".to_string(),
            },
        ],
    );

    let result = provider
        .execute_detailed(&hello_messages(), &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_some());
    assert!(result.exhausted_error.is_none());
    let real_attempts: Vec<&FallbackAttempt> = result
        .attempts
        .iter()
        .filter(|a| a.error.as_deref() != Some("skipped: in cooldown"))
        .collect();
    assert_eq!(real_attempts.len(), 2, "两级都必须是真实尝试，无 skip 占位");
    assert!(real_attempts[0].error.is_some(), "第一级真实失败");
    assert!(real_attempts[1].success, "第二级接住请求");
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
        content: "Hello".into(),
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
        content: "Hello".into(),
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
    // 冷却键 = lane 名 + 本级生效模型名（级唯一，见 cooldown_key 注记）。
    cooldown.mark_failure("p1:m1", FailoverReason::RateLimit, None);

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
        content: "Hello".into(),
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
        content: "Hello".into(),
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
        content: "Hello".into(),
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
        content: "Hello".into(),
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
        content: "Hello".into(),
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
        content: "Describe this image".into(),
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
        content: "Describe".into(),
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
        content: "Describe this image".into(),
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
        content: "Describe".into(),
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
    // 冷却键 = lane 名 + 本级生效模型名（级唯一，见 cooldown_key 注记）。
    cooldown.mark_failure("p1:m1", FailoverReason::RateLimit, None);

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
        content: "Hello".into(),
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
    // Verify it implements std::error:Error
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

// ===========================================================================
// 补测：全链冷却落空臂 / 显式 model 覆盖 / Display 明细
// ===========================================================================

/// 记录收到的 model 参数的 mock（验证 effective_model 覆盖语义）。
struct RecordingProvider {
    name: String,
    model: String,
    seen: parking_lot::Mutex<Vec<String>>,
}

#[async_trait]
impl LLMProvider for RecordingProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        self.seen.lock().push(model.to_string());
        Ok(LLMResponse {
            content: "ok".into(),
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

fn w4c_msg() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: "hi".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }]
}

#[tokio::test]
async fn test_w4c_execute_detailed_all_in_cooldown_falls_through_exhausted() {
    // 两个 provider 都在冷却 → 循环全部 continue → 落到尾部 exhausted 返回
    // （冷却键 = lane 名 + 本级生效模型名，级唯一，见 cooldown_key 注记）。
    let cooldown = Arc::new(CooldownTracker::new());
    cooldown.mark_failure("p1:m1", FailoverReason::RateLimit, None);
    cooldown.mark_failure("p2:m2", FailoverReason::RateLimit, None);

    let provider = FallbackProvider::with_cooldown(
        "all-cooled",
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

    let result = provider
        .execute_detailed(&w4c_msg(), &[], "", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    let err = result.exhausted_error.unwrap();
    assert_eq!(err.chain_name, "all-cooled");
    // 两个都被跳过，errors 空（跳过不进 errors），attempts 各带 cooldown 标记
    assert_eq!(err.errors.len(), 0);
    assert_eq!(result.attempts.len(), 2);
    assert!(result.attempts.iter().all(|a| {
        a.error
            .as_ref()
            .map(|e| e.contains("cooldown"))
            .unwrap_or(false)
    }));
}

#[tokio::test]
async fn test_w4c_execute_detailed_entry_model_wins_per_level() {
    // 回归锁：每级恒用本级装配模型名（entry.model）——
    // 调用方请求名只作级名缺失时的兜底。旧语义（非空请求名覆盖所有级）
    // 在生产装配形态下把主模型名发进备用 provider（真实上游 400）。
    let recorder = Arc::new(RecordingProvider {
        name: "rec".to_string(),
        model: "entry-model".to_string(),
        seen: parking_lot::Mutex::new(Vec::new()),
    });
    let provider = FallbackProvider::new(
        "ov",
        vec![FallbackEntry {
            provider: recorder.clone(),
            model: "entry-model".to_string(),
        }],
    );
    let result = provider
        .execute_detailed(&w4c_msg(), &[], "explicit-model", &ChatOptions::default())
        .await;
    assert!(result.response.is_some());
    // 请求名被忽略，级装配名生效
    assert_eq!(recorder.seen.lock()[0], "entry-model");
}

#[test]
fn test_w4c_fallback_exhausted_display_lists_provider_errors() {
    let e = FallbackExhaustedError {
        chain_name: "chain-x".to_string(),
        providers_attempted: 2,
        total_providers: 2,
        errors: vec![
            ("pa".to_string(), "boom-a".to_string()),
            ("pb".to_string(), "boom-b".to_string()),
        ],
    };
    let s = format!("{}", e);
    assert!(s.contains("chain 'chain-x' exhausted: 2/2"));
    assert!(s.contains("- pa: boom-a"));
    assert!(s.contains("- pb: boom-b"));
}

#[tokio::test]
async fn test_w4c_chat_trait_method_uses_entry_model_per_level() {
    // LLMProvider::chat 走同样链路（走 trait 对象而非 execute_detailed）；
    // 同款语义：级装配名优先于调用方请求名。
    let recorder = Arc::new(RecordingProvider {
        name: "rec".to_string(),
        model: "entry-model".to_string(),
        seen: parking_lot::Mutex::new(Vec::new()),
    });
    let provider = FallbackProvider::new(
        "trait",
        vec![FallbackEntry {
            provider: recorder.clone(),
            model: "entry-model".to_string(),
        }],
    );
    use crate::router::LLMProvider as _;
    let resp = provider
        .chat(&w4c_msg(), &[], "trait-model", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "ok");
    assert_eq!(recorder.seen.lock()[0], "entry-model");
}

// ---------------------------------------------------------------------------
// 跨模型 fallback 链的级模型名回归锁
// ---------------------------------------------------------------------------

/// 生产装配形态回归锁：主级失败轮到 fallback 级时，
/// fallback 级必须收到**自己的**装配模型名，而不是调用方传的主模型别名。
/// 旧语义（非空调用方名覆盖所有级）把主模型名发进备用 provider 的
/// api_base——真实上游 400 model not exist，`fallback_to` 跨模型故障转移
/// 在生产恒失效；mock 不模拟 400 所以该矛盾在测试面从未暴露。
#[tokio::test]
async fn test_fallback_chain_fallback_level_receives_own_model_name() {
    let primary = Arc::new(MockFailProvider {
        name: "p1".to_string(),
        model: "m1".to_string(),
        retriable: true,
    });
    let recorder = Arc::new(RecordingProvider {
        name: "p2".to_string(),
        model: "m2".to_string(),
        seen: parking_lot::Mutex::new(Vec::new()),
    });
    let provider = FallbackProvider::new(
        "xmodel",
        vec![
            FallbackEntry {
                provider: primary as Arc<dyn LLMProvider>,
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: recorder.clone() as Arc<dyn LLMProvider>,
                model: "m2".to_string(),
            },
        ],
    );
    use crate::router::LLMProvider as _;
    // 调用方传主模型别名（生产 ProviderAdapter 恒传非空主名）
    let resp = provider
        .chat(&w4c_msg(), &[], "m1", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "ok");
    // 回归点：fallback 级收到自己的装配名 "m2"，而非调用方请求名 "m1"
    assert_eq!(recorder.seen.lock()[0], "m2");
}

// ---------------------------------------------------------------------------
// Retry-After hint 从错误流入 cooldown
// ---------------------------------------------------------------------------

/// 429 + Retry-After 的 provider：冷却应取服务端权威 30s，而非公式第 1 次
/// 失败的 60s。
#[tokio::test]
async fn test_fallback_cooldown_honors_retry_after_hint() {
    use std::time::Duration;

    struct RetryAfterProvider {
        name: String,
    }

    #[async_trait]
    impl LLMProvider for RetryAfterProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _options: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            Err(FailoverError::RateLimit {
                provider: self.name.clone(),
                model: "m".to_string(),
                retry_after: Some(30),
            })
        }

        fn default_model(&self) -> &str {
            "m"
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    let cooldown = Arc::new(CooldownTracker::new());
    let provider = FallbackProvider::with_cooldown(
        "test-fallback",
        vec![FallbackEntry {
            provider: Arc::new(RetryAfterProvider {
                name: "ra1".to_string(),
            }),
            model: "m".to_string(),
        }],
        Arc::clone(&cooldown),
    );

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let _ = provider
        .chat(&messages, &[], "", &ChatOptions::default())
        .await;

    let remaining = cooldown
        .cooldown_remaining("ra1:m")
        .expect("provider should be in cooldown");
    assert!(
        remaining <= Duration::from_secs(30) && remaining > Duration::from_secs(28),
        "hint 30s expected, got {:?}",
        remaining
    );
}

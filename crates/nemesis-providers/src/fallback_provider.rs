//! FallbackProvider with provider chain and auto-retry.

use crate::cooldown::CooldownTracker;
use crate::error_classifier::classify_error;
use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use std::sync::Arc;
use tracing;

/// A single entry in the fallback chain.
pub struct FallbackEntry {
    pub provider: Arc<dyn LLMProvider>,
    pub model: String,
}

/// Record of a single fallback attempt.
#[derive(Debug, Clone)]
pub struct FallbackAttempt {
    /// Provider name that was tried.
    pub provider: String,
    /// Model used in this attempt.
    pub model: String,
    /// Error that occurred, if any.
    pub error: Option<String>,
    /// Whether this attempt succeeded.
    pub success: bool,
}

/// Result of a fallback chain execution with detailed attempt information.
#[derive(Debug)]
pub struct FallbackResult {
    /// The final response if successful.
    pub response: Option<LLMResponse>,
    /// All attempts made during the fallback chain.
    pub attempts: Vec<FallbackAttempt>,
    /// The final error if all attempts failed.
    pub exhausted_error: Option<FallbackExhaustedError>,
}

/// Detailed error returned when all providers in the fallback chain have been exhausted.
#[derive(Debug, Clone)]
pub struct FallbackExhaustedError {
    /// Name of the fallback chain.
    pub chain_name: String,
    /// Total number of providers attempted.
    pub providers_attempted: usize,
    /// Total number of providers in the chain.
    pub total_providers: usize,
    /// Per-provider error details.
    pub errors: Vec<(String, String)>,
}

impl std::fmt::Display for FallbackExhaustedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fallback chain '{}' exhausted: {}/{} providers attempted",
            self.chain_name, self.providers_attempted, self.total_providers
        )?;
        for (provider, error) in &self.errors {
            write!(f, "\n  - {}: {}", provider, error)?;
        }
        Ok(())
    }
}

impl std::error::Error for FallbackExhaustedError {}

/// Provider that tries multiple providers in order, with cooldown tracking
/// and automatic failover on retriable errors.
pub struct FallbackProvider {
    chain: Vec<FallbackEntry>,
    cooldown: Arc<CooldownTracker>,
    name: String,
}

impl FallbackProvider {
    /// Create a new fallback provider with the given chain.
    /// Providers are tried in order; unavailable ones (in cooldown) are skipped.
    pub fn new(name: &str, chain: Vec<FallbackEntry>) -> Self {
        Self {
            chain,
            cooldown: Arc::new(CooldownTracker::new()),
            name: name.to_string(),
        }
    }

    /// Create with a shared cooldown tracker.
    pub fn with_cooldown(name: &str, chain: Vec<FallbackEntry>, cooldown: Arc<CooldownTracker>) -> Self {
        Self {
            chain,
            cooldown,
            name: name.to_string(),
        }
    }

    /// Get the cooldown tracker reference.
    pub fn cooldown(&self) -> Arc<CooldownTracker> {
        Arc::clone(&self.cooldown)
    }

    /// Get the number of entries in the chain.
    pub fn chain_len(&self) -> usize {
        self.chain.len()
    }

    /// Execute the fallback chain for image/vision requests.
    ///
    /// Simpler than `execute_detailed`: no cooldown checks (image endpoints
    /// have different rate limits). Image dimension/size errors abort immediately
    /// (non-retriable).
    pub async fn execute_image(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> FallbackResult {
        let mut attempts = Vec::new();
        let mut errors = Vec::new();

        if self.chain.is_empty() {
            return FallbackResult {
                response: None,
                attempts,
                exhausted_error: Some(FallbackExhaustedError {
                    chain_name: self.name.clone(),
                    providers_attempted: 0,
                    total_providers: 0,
                    errors: vec![],
                }),
            };
        }

        for (i, entry) in self.chain.iter().enumerate() {
            let provider_name = entry.provider.name();
            let effective_model = if model.is_empty() { &entry.model } else { model };

            match entry.provider.chat(messages, tools, effective_model, options).await {
                Ok(resp) => {
                    attempts.push(FallbackAttempt {
                        provider: provider_name.to_string(),
                        model: effective_model.to_string(),
                        error: None,
                        success: true,
                    });
                    return FallbackResult {
                        response: Some(resp),
                        attempts,
                        exhausted_error: None,
                    };
                }
                Err(err) => {
                    let error_msg = format!("{}", err);
                    errors.push((provider_name.to_string(), error_msg.clone()));

                    // Image dimension/size errors are non-retriable.
                    let msg_lower = error_msg.to_lowercase();
                    if crate::error_classifier::is_image_dimension_error(&msg_lower)
                        || crate::error_classifier::is_image_size_error(&msg_lower)
                    {
                        attempts.push(FallbackAttempt {
                            provider: provider_name.to_string(),
                            model: effective_model.to_string(),
                            error: Some(error_msg),
                            success: false,
                        });
                        return FallbackResult {
                            response: None,
                            attempts,
                            exhausted_error: Some(FallbackExhaustedError {
                                chain_name: self.name.clone(),
                                providers_attempted: errors.len(),
                                total_providers: self.chain.len(),
                                errors,
                            }),
                        };
                    }

                    attempts.push(FallbackAttempt {
                        provider: provider_name.to_string(),
                        model: effective_model.to_string(),
                        error: Some(error_msg),
                        success: false,
                    });

                    // If this was the last candidate, return exhausted.
                    if i == self.chain.len() - 1 {
                        return FallbackResult {
                            response: None,
                            attempts,
                            exhausted_error: Some(FallbackExhaustedError {
                                chain_name: self.name.clone(),
                                providers_attempted: errors.len(),
                                total_providers: self.chain.len(),
                                errors,
                            }),
                        };
                    }
                }
            }
        }

        FallbackResult {
            response: None,
            attempts,
            exhausted_error: Some(FallbackExhaustedError {
                chain_name: self.name.clone(),
                providers_attempted: errors.len(),
                total_providers: self.chain.len(),
                errors,
            }),
        }
    }

    /// Resolve fallback candidates from a configuration string.
    ///
    /// Takes a comma-separated list of "provider/model" entries and deduplicates them.
    pub fn resolve_candidates<'a>(chain: &'a [FallbackEntry], _model_override: &str) -> Vec<&'a FallbackEntry> {
        let mut seen = std::collections::HashSet::new();
        chain
            .iter()
            .filter(|entry| {
                let key = format!("{}:{}", entry.provider.name(), entry.model);
                if seen.contains(&key) {
                    false
                } else {
                    seen.insert(key);
                    true
                }
            })
            .collect()
    }

    /// Execute the fallback chain with detailed attempt tracking.
    pub async fn execute_detailed(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> FallbackResult {
        let mut attempts = Vec::new();
        let mut errors = Vec::new();

        if self.chain.is_empty() {
            return FallbackResult {
                response: None,
                attempts,
                exhausted_error: Some(FallbackExhaustedError {
                    chain_name: self.name.clone(),
                    providers_attempted: 0,
                    total_providers: 0,
                    errors: vec![],
                }),
            };
        }

        for entry in &self.chain {
            let provider_name = entry.provider.name();
            let effective_model = if model.is_empty() { &entry.model } else { model };

            if !self.cooldown.is_available(provider_name) {
                attempts.push(FallbackAttempt {
                    provider: provider_name.to_string(),
                    model: effective_model.to_string(),
                    error: Some("skipped: in cooldown".to_string()),
                    success: false,
                });
                continue;
            }

            match entry.provider.chat(messages, tools, effective_model, options).await {
                Ok(resp) => {
                    self.cooldown.mark_success(provider_name);
                    attempts.push(FallbackAttempt {
                        provider: provider_name.to_string(),
                        model: effective_model.to_string(),
                        error: None,
                        success: true,
                    });
                    return FallbackResult {
                        response: Some(resp),
                        attempts,
                        exhausted_error: None,
                    };
                }
                Err(err) => {
                    let reason = err.reason();
                    self.cooldown.mark_failure(provider_name, reason);
                    let error_msg = format!("{}", err);
                    errors.push((provider_name.to_string(), error_msg.clone()));
                    attempts.push(FallbackAttempt {
                        provider: provider_name.to_string(),
                        model: effective_model.to_string(),
                        error: Some(error_msg),
                        success: false,
                    });

                    if !err.is_retriable() {
                        return FallbackResult {
                            response: None,
                            attempts,
                            exhausted_error: Some(FallbackExhaustedError {
                                chain_name: self.name.clone(),
                                providers_attempted: errors.len(),
                                total_providers: self.chain.len(),
                                errors,
                            }),
                        };
                    }
                }
            }
        }

        FallbackResult {
            response: None,
            attempts,
            exhausted_error: Some(FallbackExhaustedError {
                chain_name: self.name.clone(),
                providers_attempted: errors.len(),
                total_providers: self.chain.len(),
                errors,
            }),
        }
    }
}

#[async_trait]
impl LLMProvider for FallbackProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        if self.chain.is_empty() {
            return Err(FailoverError::Unknown {
                provider: self.name.clone(),
                message: "no providers in fallback chain".to_string(),
            });
        }

        let mut last_error: Option<FailoverError> = None;

        for entry in &self.chain {
            let provider_name = entry.provider.name();

            // Skip providers in cooldown
            if !self.cooldown.is_available(provider_name) {
                continue;
            }

            let effective_model = if model.is_empty() {
                &entry.model
            } else {
                model
            };

            match entry
                .provider
                .chat(messages, tools, effective_model, options)
                .await
            {
                Ok(resp) => {
                    self.cooldown.mark_success(provider_name);
                    return Ok(resp);
                }
                Err(err) => {
                    let reason = err.reason();
                    self.cooldown.mark_failure(provider_name, reason);

                    // Classify the error for additional context
                    let error_msg = format!("{}", err);
                    if let Some(classified) = classify_error(&error_msg, provider_name, effective_model) {
                        tracing::warn!(
                            provider = provider_name,
                            model = effective_model,
                            reason = ?classified.reason(),
                            "provider failed, classified error"
                        );
                    }

                    if !err.is_retriable() {
                        // Non-retriable: return immediately
                        return Err(err);
                    }

                    last_error = Some(err);
                }
            }
        }

        // All providers exhausted
        Err(last_error.unwrap_or_else(|| FailoverError::Unknown {
            provider: self.name.clone(),
            message: "all providers in fallback chain exhausted".to_string(),
        }))
    }

    fn default_model(&self) -> &str {
        self.chain
            .first()
            .map(|e| e.provider.default_model())
            .unwrap_or("")
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
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
            FallbackEntry { provider: p1.clone(), model: "m1".to_string() },
            FallbackEntry { provider: p1.clone(), model: "m1".to_string() },
            FallbackEntry { provider: Arc::new(MockSuccessProvider { name: "p2".to_string(), model: "m2".to_string() }), model: "m2".to_string() },
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
            fn default_model(&self) -> &str { &self.model }
            fn name(&self) -> &str { &self.name }
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
            fn default_model(&self) -> &str { &self.model }
            fn name(&self) -> &str { &self.name }
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
        }];

        let result = provider
            .execute_detailed(&messages, &[], "", &ChatOptions::default())
            .await;
        // p1 should be skipped (in cooldown), p2 should succeed
        assert!(result.response.is_some());
        assert!(result.attempts.iter().any(|a| a.provider == "p1" && a.error.as_ref().unwrap().contains("cooldown")));
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
        let chain: Vec<FallbackEntry> = (1..=5).map(|i| {
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider {
                    name: format!("p{}", i),
                    model: format!("m{}", i),
                }),
                model: format!("m{}", i),
            }
        }).collect();

        let candidates = FallbackProvider::resolve_candidates(&chain, "");
        assert_eq!(candidates.len(), 5);
        assert_eq!(candidates[0].provider.name(), "p1");
        assert_eq!(candidates[4].provider.name(), "p5");
    }
}

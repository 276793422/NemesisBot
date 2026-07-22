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
        tracing::info!(
            name = name,
            chain_length = chain.len(),
            "[Provider] Created fallback provider chain"
        );
        Self {
            chain,
            cooldown: Arc::new(CooldownTracker::new()),
            name: name.to_string(),
        }
    }

    /// Create with a shared cooldown tracker.
    pub fn with_cooldown(
        name: &str,
        chain: Vec<FallbackEntry>,
        cooldown: Arc<CooldownTracker>,
    ) -> Self {
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
    pub fn resolve_candidates<'a>(
        chain: &'a [FallbackEntry],
        _model_override: &str,
    ) -> Vec<&'a FallbackEntry> {
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
            let effective_model = if model.is_empty() {
                &entry.model
            } else {
                model
            };

            if !self.cooldown.is_available(provider_name) {
                tracing::debug!(
                    provider = provider_name,
                    "[Provider] Skipping provider in cooldown during detailed execution"
                );
                attempts.push(FallbackAttempt {
                    provider: provider_name.to_string(),
                    model: effective_model.to_string(),
                    error: Some("skipped: in cooldown".to_string()),
                    success: false,
                });
                continue;
            }

            match entry
                .provider
                .chat(messages, tools, effective_model, options)
                .await
            {
                Ok(resp) => {
                    self.cooldown.mark_success(provider_name);
                    tracing::info!(
                        provider = provider_name,
                        model = effective_model,
                        "[Provider] Fallback chain succeeded"
                    );
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
                    tracing::warn!(
                        provider = provider_name,
                        model = effective_model,
                        error = %error_msg,
                        retriable = err.is_retriable(),
                        "[Provider] Fallback attempt failed"
                    );
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
                tracing::debug!(
                    provider = provider_name,
                    "[Provider] Skipping provider in cooldown"
                );
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
                    if let Some(classified) =
                        classify_error(&error_msg, provider_name, effective_model)
                    {
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
mod tests;

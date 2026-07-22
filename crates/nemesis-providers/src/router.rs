//! Provider Router - intelligent model selection and routing.

use crate::failover::FailoverError;
use crate::types::*;
use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Selection policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Cost,
    Quality,
    Latency,
    RoundRobin,
    Fallback,
}

impl Default for Policy {
    fn default() -> Self {
        Policy::Fallback
    }
}

/// Policy configuration describing a named routing policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Policy name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// The policy variant.
    pub policy: Policy,
    /// Optional weights for balanced policies.
    #[serde(default)]
    pub weights: PolicyWeights,
}

/// Get the built-in default alias mappings.
///
/// Short names map to full provider/model strings, allowing users to
/// reference models by simple names like "fast" or "smart".
pub fn default_aliases() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert(
        "fast".to_string(),
        "groq/llama-3.3-70b-versatile".to_string(),
    );
    map.insert(
        "smart".to_string(),
        "anthropic/claude-sonnet-4-20250514".to_string(),
    );
    map.insert("cheap".to_string(), "deepseek/deepseek-chat".to_string());
    map.insert("local".to_string(), "ollama/llama3.3".to_string());
    map.insert("reasoning".to_string(), "openai/o3-mini".to_string());
    map.insert(
        "code".to_string(),
        "anthropic/claude-sonnet-4-20250514".to_string(),
    );
    map
}

/// Resolve a short name through an alias map.
///
/// If the name is found in the map, returns the mapped value.
/// Otherwise returns the original name unchanged (returned via `None`).
pub fn resolve_alias(aliases: &HashMap<String, String>, name: &str) -> Option<String> {
    aliases.get(name).cloned()
}

/// Merge custom aliases with defaults.
///
/// Custom aliases take precedence over defaults when keys collide.
/// Neither input map is modified; a new map is returned.
pub fn merge_aliases(
    defaults: &HashMap<String, String>,
    custom: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut result = defaults.clone();
    for (k, v) in custom {
        result.insert(k.clone(), v.clone());
    }
    result
}

/// Predefined policy configurations.
fn predefined_policies() -> HashMap<String, PolicyConfig> {
    let mut map = HashMap::new();
    map.insert("fast".to_string(), PolicyConfig {
        name: "fast".to_string(),
        description: "Optimize for lowest latency. Selects the provider with the fastest response time based on recorded metrics.".to_string(),
        policy: Policy::Latency,
        weights: PolicyWeights { cost: 0.0, quality: 0.0, latency: 1.0 },
    });
    map.insert("balanced".to_string(), PolicyConfig {
        name: "balanced".to_string(),
        description: "Balance cost, quality, and latency equally. Uses weighted scoring across all three factors.".to_string(),
        policy: Policy::Quality,
        weights: PolicyWeights { cost: 0.33, quality: 0.34, latency: 0.33 },
    });
    map.insert(
        "cheap".to_string(),
        PolicyConfig {
            name: "cheap".to_string(),
            description:
                "Optimize for lowest cost. Always selects the cheapest provider per 1K tokens."
                    .to_string(),
            policy: Policy::Cost,
            weights: PolicyWeights {
                cost: 1.0,
                quality: 0.0,
                latency: 0.0,
            },
        },
    );
    map.insert("best".to_string(), PolicyConfig {
        name: "best".to_string(),
        description: "Optimize for highest quality. Always selects the provider with the highest quality score.".to_string(),
        policy: Policy::Quality,
        weights: PolicyWeights { cost: 0.0, quality: 1.0, latency: 0.0 },
    });
    map
}

/// Get a named policy configuration.
///
/// Returns the "balanced" policy if the name is not recognized.
pub fn get_policy(name: &str) -> PolicyConfig {
    let policies = predefined_policies();
    match policies.get(name) {
        Some(p) => p.clone(),
        None => policies
            .get("balanced")
            .expect("balanced always exists")
            .clone(),
    }
}

/// Get all predefined policy configurations.
pub fn all_policies() -> HashMap<String, PolicyConfig> {
    predefined_policies()
}

/// Get the names of all predefined policies.
pub fn policy_names() -> Vec<String> {
    predefined_policies().keys().cloned().collect()
}

/// Router configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    #[serde(default)]
    pub default_policy: Policy,
    #[serde(default)]
    pub aliases: HashMap<String, String>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        let mut aliases = HashMap::new();
        aliases.insert("fast".to_string(), "groq/llama-3".to_string());
        aliases.insert(
            "smart".to_string(),
            "anthropic/claude-sonnet-4-6".to_string(),
        );
        aliases.insert("cheap".to_string(), "deepseek/deepseek-chat".to_string());
        aliases.insert("local".to_string(), "ollama/llama3".to_string());
        Self {
            default_policy: Policy::Fallback,
            aliases,
        }
    }
}

/// A routing candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub cost_per_1k: f64,
    #[serde(default = "default_quality")]
    pub quality_score: f64,
    #[serde(default)]
    pub priority: i32,
}

fn default_quality() -> f64 {
    0.5
}

/// Policy weights for weighted selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyWeights {
    #[serde(default = "default_weight")]
    pub cost: f64,
    #[serde(default = "default_weight")]
    pub quality: f64,
    #[serde(default = "default_weight")]
    pub latency: f64,
}

fn default_weight() -> f64 {
    0.33
}

impl Default for PolicyWeights {
    fn default() -> Self {
        Self {
            cost: 0.33,
            quality: 0.33,
            latency: 0.34,
        }
    }
}

/// A single metric sample.
#[derive(Debug, Clone)]
pub struct Metric {
    pub provider: String,
    pub latency_ms: u64,
    pub success: bool,
    pub tokens_used: i64,
    pub cost: f64,
    pub timestamp: chrono::DateTime<chrono::Local>,
}

/// Aggregated provider metrics.
#[derive(Debug, Clone, Default)]
pub struct ProviderMetrics {
    pub provider: String,
    pub avg_latency_ms: f64,
    pub success_rate: f64,
    pub total_requests: i64,
    pub total_failures: i64,
    pub avg_cost_per_1k: f64,
}

/// Ring buffer metrics collector.
pub struct MetricsCollector {
    samples: DashMap<String, Vec<Metric>>,
    max_per_provider: usize,
}

impl MetricsCollector {
    pub fn new(max_per_provider: usize) -> Self {
        Self {
            samples: DashMap::new(),
            max_per_provider,
        }
    }

    /// Record a metric sample.
    pub fn record(&self, metric: Metric) {
        let mut entry = self
            .samples
            .entry(metric.provider.clone())
            .or_insert_with(Vec::new);
        if entry.len() >= self.max_per_provider {
            entry.remove(0);
        }
        entry.push(metric);
    }

    /// Get aggregated metrics for a provider.
    pub fn get_metrics(&self, provider: &str) -> ProviderMetrics {
        let entry = self.samples.get(provider);
        let samples = match entry {
            Some(s) => s,
            None => {
                return ProviderMetrics {
                    provider: provider.to_string(),
                    ..Default::default()
                };
            }
        };

        if samples.is_empty() {
            return ProviderMetrics {
                provider: provider.to_string(),
                ..Default::default()
            };
        }

        let total = samples.len() as i64;
        let successes = samples.iter().filter(|s| s.success).count();
        let avg_latency = samples.iter().map(|s| s.latency_ms as f64).sum::<f64>() / total as f64;
        let total_tokens: i64 = samples.iter().map(|s| s.tokens_used).sum();
        let total_cost: f64 = samples.iter().map(|s| s.cost).sum();

        ProviderMetrics {
            provider: provider.to_string(),
            avg_latency_ms: avg_latency,
            success_rate: successes as f64 / total as f64,
            total_requests: total,
            total_failures: total - successes as i64,
            avg_cost_per_1k: if total_tokens > 0 {
                total_cost / (total_tokens as f64 / 1000.0)
            } else {
                0.0
            },
        }
    }

    /// Get aggregated metrics for all providers.
    pub fn get_all_metrics(&self) -> HashMap<String, ProviderMetrics> {
        let providers: Vec<String> = self.samples.iter().map(|e| e.key().clone()).collect();
        providers
            .into_iter()
            .map(|p| (p.clone(), self.get_metrics(&p)))
            .collect()
    }

    /// Reset (clear) all recorded metrics for a specific provider.
    pub fn reset(&self, provider: &str) {
        self.samples.remove(provider);
    }

    /// Prune (remove) samples older than the given duration from all providers.
    pub fn prune(&self, older_than: std::time::Duration) {
        let cutoff = chrono::Local::now()
            - chrono::Duration::from_std(older_than).unwrap_or(chrono::Duration::seconds(0));
        let providers: Vec<String> = self.samples.iter().map(|e| e.key().clone()).collect();
        for provider in providers {
            if let Some(mut entry) = self.samples.get_mut(&provider) {
                let original_len = entry.len();
                entry.retain(|s| s.timestamp > cutoff);
                let removed = original_len - entry.len();
                if removed > 0 {
                    tracing::debug!(
                        provider = %provider,
                        removed = removed,
                        "Pruned old metric samples"
                    );
                }
            }
        }
    }
}

/// LLM Provider trait.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Send a chat completion request.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError>;

    /// Get the default model for this provider.
    fn default_model(&self) -> &str;

    /// Get the provider name.
    fn name(&self) -> &str;
}

/// Provider router with intelligent model selection.
pub struct Router {
    providers: DashMap<String, Arc<dyn LLMProvider>>,
    candidates: RwLock<Vec<Candidate>>,
    metrics: Arc<MetricsCollector>,
    config: RwLock<RouterConfig>,
    rr_counters: DashMap<String, AtomicU64>,
}

impl Router {
    pub fn new(config: RouterConfig) -> Self {
        Self {
            providers: DashMap::new(),
            candidates: RwLock::new(Vec::new()),
            metrics: Arc::new(MetricsCollector::new(1000)),
            config: RwLock::new(config),
            rr_counters: DashMap::new(),
        }
    }

    /// Register a provider.
    pub fn register_provider(&self, name: &str, provider: Arc<dyn LLMProvider>) {
        tracing::info!(
            provider = name,
            default_model = %provider.default_model(),
            "[Provider] Registered provider"
        );
        self.providers.insert(name.to_string(), provider);
    }

    /// Add a routing candidate.
    pub fn add_candidate(&self, candidate: Candidate) {
        self.candidates.write().push(candidate);
    }

    /// Resolve a model alias to actual model name.
    pub fn resolve_alias(&self, model: &str) -> String {
        let config = self.config.read();
        if let Some(resolved) = config.aliases.get(model) {
            resolved.clone()
        } else {
            model.to_string()
        }
    }

    /// Select the best candidate based on the current policy.
    pub fn select(&self, model: &str) -> Option<Candidate> {
        let resolved = self.resolve_alias(model);
        let candidates = self.candidates.read();

        // Filter candidates that match the requested model
        let matching: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.model == resolved || resolved.contains('/') && c.model == resolved)
            .collect();

        if matching.is_empty() {
            // Return first candidate as fallback
            return candidates.first().cloned();
        }

        if matching.len() == 1 {
            return Some(matching[0].clone());
        }

        let policy = self.config.read().default_policy;
        self.select_by_policy(&policy, &matching)
    }

    /// Select the best candidate using an explicit policy override.
    ///
    /// Mirrors Go's `Router.SelectWithPolicy`. The given policy overrides
    /// the default routing policy for this single selection.
    pub fn select_with_policy(&self, policy: Policy, model: &str) -> Option<Candidate> {
        let resolved = self.resolve_alias(model);
        let candidates = self.candidates.read();

        // Filter candidates that match the requested model
        let matching: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.model == resolved || resolved.contains('/') && c.model == resolved)
            .collect();

        if matching.is_empty() {
            return candidates.first().cloned();
        }

        if matching.len() == 1 {
            return Some(matching[0].clone());
        }

        self.select_by_policy(&policy, &matching)
    }

    /// Internal selection by policy applied to a slice of matching candidates.
    fn select_by_policy(&self, policy: &Policy, matching: &[&Candidate]) -> Option<Candidate> {
        match policy {
            Policy::Cost => matching
                .iter()
                .min_by(|a, b| a.cost_per_1k.partial_cmp(&b.cost_per_1k).unwrap())
                .cloned()
                .cloned(),
            Policy::Quality => matching
                .iter()
                .max_by(|a, b| a.quality_score.partial_cmp(&b.quality_score).unwrap())
                .cloned()
                .cloned(),
            Policy::Latency => {
                let metrics = self.metrics.clone();
                matching
                    .iter()
                    .min_by(|a, b| {
                        let ma = metrics.get_metrics(&a.provider);
                        let mb = metrics.get_metrics(&b.provider);
                        ma.avg_latency_ms.partial_cmp(&mb.avg_latency_ms).unwrap()
                    })
                    .cloned()
                    .cloned()
            }
            Policy::RoundRobin => {
                if let Some(first) = matching.first() {
                    let counter = self
                        .rr_counters
                        .entry(first.provider.clone())
                        .or_insert_with(|| AtomicU64::new(0));
                    let idx = counter.fetch_add(1, Ordering::Relaxed) as usize % matching.len();
                    Some(matching[idx].clone())
                } else {
                    None
                }
            }
            Policy::Fallback => matching
                .iter()
                .max_by(|a, b| a.priority.cmp(&b.priority))
                .cloned()
                .cloned(),
        }
    }

    /// Set the default routing policy.
    ///
    /// Mirrors Go's `Router.SetPolicy`.
    pub fn set_policy(&self, policy: Policy) {
        self.config.write().default_policy = policy;
    }

    /// Get the current default routing policy.
    ///
    /// Mirrors Go's `Router.GetPolicy`.
    pub fn get_policy(&self) -> Policy {
        self.config.read().default_policy
    }

    /// Update the alias mappings.
    ///
    /// Mirrors Go's `Router.SetAliases`. Replaces all existing aliases.
    pub fn set_aliases(&self, aliases: HashMap<String, String>) {
        self.config.write().aliases = aliases;
    }

    /// Chat with automatic provider selection and failover.
    pub async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let resolved = self.resolve_alias(model);

        // Try primary
        if let Some(candidate) = self.select(&resolved) {
            if let Some(provider) = self.providers.get(&candidate.provider) {
                let start = std::time::Instant::now();
                match provider
                    .chat(messages, tools, &candidate.model, options)
                    .await
                {
                    Ok(resp) => {
                        self.metrics.record(Metric {
                            provider: candidate.provider.clone(),
                            latency_ms: start.elapsed().as_millis() as u64,
                            success: true,
                            tokens_used: resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
                            cost: candidate.cost_per_1k
                                * resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0) as f64
                                / 1000.0,
                            timestamp: chrono::Local::now(),
                        });
                        return Ok(resp);
                    }
                    Err(e) => {
                        tracing::warn!(
                            provider = %candidate.provider,
                            model = %candidate.model,
                            error = %e,
                            "[Provider] Primary provider failed, attempting fallback"
                        );
                        self.metrics.record(Metric {
                            provider: candidate.provider.clone(),
                            latency_ms: start.elapsed().as_millis() as u64,
                            success: false,
                            tokens_used: 0,
                            cost: 0.0,
                            timestamp: chrono::Local::now(),
                        });
                        if e.is_retriable() {
                            // Try fallback candidates
                            let candidates = self.candidates.read();
                            for alt in candidates
                                .iter()
                                .filter(|c| c.model == resolved && c.provider != candidate.provider)
                            {
                                if let Some(alt_provider) = self.providers.get(&alt.provider) {
                                    tracing::warn!(
                                        from_provider = %candidate.provider,
                                        to_provider = %alt.provider,
                                        model = %alt.model,
                                        "[Provider] Fallback triggered"
                                    );
                                    match alt_provider
                                        .chat(messages, tools, &alt.model, options)
                                        .await
                                    {
                                        Ok(resp) => return Ok(resp),
                                        Err(_) => continue,
                                    }
                                }
                            }
                        }
                        return Err(e);
                    }
                }
            }
        }

        Err(FailoverError::Unknown {
            provider: "router".to_string(),
            message: format!("no provider available for model: {}", resolved),
        })
    }

    /// Get the metrics collector.
    pub fn metrics(&self) -> Arc<MetricsCollector> {
        Arc::clone(&self.metrics)
    }
}

#[cfg(test)]
mod tests;

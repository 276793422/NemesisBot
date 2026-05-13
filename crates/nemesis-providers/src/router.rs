//! Provider Router - intelligent model selection and routing.

use crate::failover::FailoverError;
use crate::types::*;
use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    map.insert("fast".to_string(), "groq/llama-3.3-70b-versatile".to_string());
    map.insert("smart".to_string(), "anthropic/claude-sonnet-4-20250514".to_string());
    map.insert("cheap".to_string(), "deepseek/deepseek-chat".to_string());
    map.insert("local".to_string(), "ollama/llama3.3".to_string());
    map.insert("reasoning".to_string(), "openai/o3-mini".to_string());
    map.insert("code".to_string(), "anthropic/claude-sonnet-4-20250514".to_string());
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
    map.insert("cheap".to_string(), PolicyConfig {
        name: "cheap".to_string(),
        description: "Optimize for lowest cost. Always selects the cheapest provider per 1K tokens.".to_string(),
        policy: Policy::Cost,
        weights: PolicyWeights { cost: 1.0, quality: 0.0, latency: 0.0 },
    });
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
        None => policies.get("balanced").expect("balanced always exists").clone(),
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
        aliases.insert("smart".to_string(), "anthropic/claude-sonnet-4-6".to_string());
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
    pub timestamp: chrono::DateTime<chrono::Utc>,
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
        let mut entry = self.samples.entry(metric.provider.clone()).or_insert_with(Vec::new);
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
            None => return ProviderMetrics {
                provider: provider.to_string(),
                ..Default::default()
            },
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
            avg_cost_per_1k: if total_tokens > 0 { total_cost / (total_tokens as f64 / 1000.0) } else { 0.0 },
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
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(older_than).unwrap_or(chrono::Duration::seconds(0));
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
            Policy::Cost => matching.iter().min_by(|a, b| a.cost_per_1k.partial_cmp(&b.cost_per_1k).unwrap()).cloned().cloned(),
            Policy::Quality => matching.iter().max_by(|a, b| a.quality_score.partial_cmp(&b.quality_score).unwrap()).cloned().cloned(),
            Policy::Latency => {
                let metrics = self.metrics.clone();
                matching.iter().min_by(|a, b| {
                    let ma = metrics.get_metrics(&a.provider);
                    let mb = metrics.get_metrics(&b.provider);
                    ma.avg_latency_ms.partial_cmp(&mb.avg_latency_ms).unwrap()
                }).cloned().cloned()
            }
            Policy::RoundRobin => {
                if let Some(first) = matching.first() {
                    let counter = self.rr_counters.entry(first.provider.clone()).or_insert_with(|| AtomicU64::new(0));
                    let idx = counter.fetch_add(1, Ordering::Relaxed) as usize % matching.len();
                    Some(matching[idx].clone())
                } else {
                    None
                }
            }
            Policy::Fallback => matching.iter().max_by(|a, b| a.priority.cmp(&b.priority)).cloned().cloned(),
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
                match provider.chat(messages, tools, &candidate.model, options).await {
                    Ok(resp) => {
                        self.metrics.record(Metric {
                            provider: candidate.provider.clone(),
                            latency_ms: start.elapsed().as_millis() as u64,
                            success: true,
                            tokens_used: resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
                            cost: candidate.cost_per_1k * resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0) as f64 / 1000.0,
                            timestamp: chrono::Utc::now(),
                        });
                        return Ok(resp);
                    }
                    Err(e) => {
                        self.metrics.record(Metric {
                            provider: candidate.provider.clone(),
                            latency_ms: start.elapsed().as_millis() as u64,
                            success: false,
                            tokens_used: 0,
                            cost: 0.0,
                            timestamp: chrono::Utc::now(),
                        });
                        if e.is_retriable() {
                            // Try fallback candidates
                            let candidates = self.candidates.read();
                            for alt in candidates.iter().filter(|c| c.model == resolved && c.provider != candidate.provider) {
                                if let Some(alt_provider) = self.providers.get(&alt.provider) {
                                    match alt_provider.chat(messages, tools, &alt.model, options).await {
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
mod tests {
    use super::*;

    #[test]
    fn test_default_aliases() {
        let config = RouterConfig::default();
        assert_eq!(config.aliases.get("fast").unwrap(), "groq/llama-3");
        assert_eq!(config.aliases.get("smart").unwrap(), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn test_resolve_alias() {
        let router = Router::new(RouterConfig::default());
        assert_eq!(router.resolve_alias("fast"), "groq/llama-3");
        assert_eq!(router.resolve_alias("gpt-4"), "gpt-4");
    }

    #[test]
    fn test_select_fallback() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Fallback,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "azure".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 2,
        });

        let selected = router.select("gpt-4").unwrap();
        assert_eq!(selected.provider, "azure"); // Higher priority
    }

    #[test]
    fn test_select_cost() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Cost,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "deepseek".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.7,
            priority: 1,
        });

        let selected = router.select("gpt-4").unwrap();
        assert_eq!(selected.provider, "deepseek"); // Cheaper
    }

    #[test]
    fn test_metrics_collector() {
        let collector = MetricsCollector::new(100);
        collector.record(Metric {
            provider: "openai".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 500,
            cost: 0.015,
            timestamp: chrono::Utc::now(),
        });
        collector.record(Metric {
            provider: "openai".to_string(),
            latency_ms: 200,
            success: false,
            tokens_used: 0,
            cost: 0.0,
            timestamp: chrono::Utc::now(),
        });

        let metrics = collector.get_metrics("openai");
        assert_eq!(metrics.total_requests, 2);
        assert_eq!(metrics.total_failures, 1);
        assert!((metrics.success_rate - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_metrics_ring_buffer() {
        let collector = MetricsCollector::new(3);
        for i in 0..5 {
            collector.record(Metric {
                provider: "test".to_string(),
                latency_ms: i * 100,
                success: true,
                tokens_used: 100,
                cost: 0.01,
                timestamp: chrono::Utc::now(),
            });
        }
        let metrics = collector.get_metrics("test");
        assert_eq!(metrics.total_requests, 3); // Only last 3 kept
    }

    #[test]
    fn test_default_aliases_function() {
        let aliases = default_aliases();
        assert_eq!(aliases.get("fast").unwrap(), "groq/llama-3.3-70b-versatile");
        assert_eq!(aliases.get("smart").unwrap(), "anthropic/claude-sonnet-4-20250514");
        assert_eq!(aliases.get("cheap").unwrap(), "deepseek/deepseek-chat");
        assert_eq!(aliases.get("local").unwrap(), "ollama/llama3.3");
    }

    #[test]
    fn test_resolve_alias_function() {
        let aliases = default_aliases();
        assert_eq!(resolve_alias(&aliases, "fast"), Some("groq/llama-3.3-70b-versatile".to_string()));
        assert_eq!(resolve_alias(&aliases, "gpt-4"), None);
    }

    #[test]
    fn test_merge_aliases_custom_overrides() {
        let defaults = default_aliases();
        let mut custom = HashMap::new();
        custom.insert("fast".to_string(), "custom/fast-model".to_string());
        custom.insert("my-custom".to_string(), "custom/model".to_string());

        let merged = merge_aliases(&defaults, &custom);
        assert_eq!(merged.get("fast").unwrap(), "custom/fast-model");
        assert_eq!(merged.get("my-custom").unwrap(), "custom/model");
        // Default still present
        assert_eq!(merged.get("smart").unwrap(), "anthropic/claude-sonnet-4-20250514");
    }

    #[test]
    fn test_get_policy_known() {
        let p = get_policy("fast");
        assert_eq!(p.policy, Policy::Latency);
        assert_eq!(p.name, "fast");
    }

    #[test]
    fn test_get_policy_unknown_returns_balanced() {
        let p = get_policy("nonexistent");
        assert_eq!(p.name, "balanced");
    }

    #[test]
    fn test_all_policies() {
        let policies = all_policies();
        assert!(policies.contains_key("fast"));
        assert!(policies.contains_key("balanced"));
        assert!(policies.contains_key("cheap"));
        assert!(policies.contains_key("best"));
        assert_eq!(policies.len(), 4);
    }

    #[test]
    fn test_policy_names() {
        let names = policy_names();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"fast".to_string()));
    }

    #[test]
    fn test_get_all_metrics() {
        let collector = MetricsCollector::new(100);
        collector.record(Metric {
            provider: "openai".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 500,
            cost: 0.015,
            timestamp: chrono::Utc::now(),
        });
        collector.record(Metric {
            provider: "anthropic".to_string(),
            latency_ms: 200,
            success: true,
            tokens_used: 300,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });

        let all = collector.get_all_metrics();
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("openai"));
        assert!(all.contains_key("anthropic"));
    }

    #[test]
    fn test_reset_metrics() {
        let collector = MetricsCollector::new(100);
        collector.record(Metric {
            provider: "openai".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 500,
            cost: 0.015,
            timestamp: chrono::Utc::now(),
        });
        assert_eq!(collector.get_metrics("openai").total_requests, 1);
        collector.reset("openai");
        assert_eq!(collector.get_metrics("openai").total_requests, 0);
    }

    #[test]
    fn test_prune_old_samples() {
        let collector = MetricsCollector::new(100);
        // Old sample (1 hour ago)
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 500,
            cost: 0.01,
            timestamp: chrono::Utc::now() - chrono::Duration::hours(2),
        });
        // Recent sample
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: 50,
            success: true,
            tokens_used: 200,
            cost: 0.005,
            timestamp: chrono::Utc::now(),
        });

        assert_eq!(collector.get_metrics("test").total_requests, 2);

        // Prune samples older than 1 hour
        collector.prune(std::time::Duration::from_secs(3600));

        let metrics = collector.get_metrics("test");
        assert_eq!(metrics.total_requests, 1);
    }

    // --- Benchmark-style throughput tests ---

    #[test]
    fn test_router_select_throughput() {
        let router = Router::new(RouterConfig::default());

        // Register candidates
        for i in 0..10 {
            router.add_candidate(Candidate {
                provider: format!("provider-{}", i),
                model: format!("model-{}", i),
                cost_per_1k: 0.01,
                quality_score: 0.9,
                priority: i as i32,
            });
        }

        let start = std::time::Instant::now();
        let iterations = 10_000;
        for i in 0..iterations {
            let _ = router.select(&format!("model-{}", i % 10));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "Router select too slow: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_metrics_collector_record_throughput() {
        let collector = MetricsCollector::new(1000);

        let start = std::time::Instant::now();
        let iterations = 10_000;
        for i in 0..iterations {
            collector.record(Metric {
                provider: format!("provider-{}", i % 5),
                latency_ms: 100 + (i % 50) as u64,
                success: i % 10 != 0,
                tokens_used: 100,
                cost: 0.001,
                timestamp: chrono::Utc::now(),
            });
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "MetricsCollector record too slow: {:?}",
            elapsed
        );
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_policy_default_is_fallback() {
        assert_eq!(Policy::default(), Policy::Fallback);
    }

    #[test]
    fn test_router_config_default() {
        let config = RouterConfig::default();
        assert_eq!(config.default_policy, Policy::Fallback);
        assert!(!config.aliases.is_empty());
    }

    #[test]
    fn test_policy_weights_default() {
        let weights = PolicyWeights::default();
        assert!((weights.cost - 0.33).abs() < 0.01);
        assert!((weights.quality - 0.33).abs() < 0.01);
        assert!((weights.latency - 0.34).abs() < 0.01);
    }

    #[test]
    fn test_select_quality_policy() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Quality,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "deepseek".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.7,
            priority: 2,
        });

        let selected = router.select("gpt-4").unwrap();
        assert_eq!(selected.provider, "openai"); // Higher quality
    }

    #[test]
    fn test_select_latency_policy() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Latency,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "fast-provider".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "slow-provider".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.7,
            priority: 2,
        });

        // Record metrics for the slow provider (higher latency)
        router.metrics().record(Metric {
            provider: "fast-provider".to_string(),
            latency_ms: 50,
            success: true,
            tokens_used: 100,
            cost: 0.001,
            timestamp: chrono::Utc::now(),
        });
        router.metrics().record(Metric {
            provider: "slow-provider".to_string(),
            latency_ms: 500,
            success: true,
            tokens_used: 100,
            cost: 0.001,
            timestamp: chrono::Utc::now(),
        });

        let selected = router.select("gpt-4").unwrap();
        assert_eq!(selected.provider, "fast-provider");
    }

    #[test]
    fn test_select_round_robin() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::RoundRobin,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "provider-a".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "provider-b".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.7,
            priority: 2,
        });

        let first = router.select("gpt-4").unwrap();
        let second = router.select("gpt-4").unwrap();
        // Round-robin should alternate
        assert_ne!(first.provider, second.provider);
    }

    #[test]
    fn test_select_no_matching_returns_first() {
        let router = Router::new(RouterConfig::default());
        router.add_candidate(Candidate {
            provider: "default".to_string(),
            model: "default-model".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.5,
            priority: 1,
        });

        let selected = router.select("nonexistent-model");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().model, "default-model");
    }

    #[test]
    fn test_select_empty_candidates() {
        let router = Router::new(RouterConfig::default());
        let selected = router.select("anything");
        assert!(selected.is_none());
    }

    #[test]
    fn test_select_with_policy_override() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Fallback,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "cheap".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.5,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "expensive".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.10,
            quality_score: 0.9,
            priority: 2,
        });

        // Default is Fallback (priority-based)
        let fb = router.select("gpt-4").unwrap();
        assert_eq!(fb.provider, "expensive");

        // Override with Quality
        let q = router.select_with_policy(Policy::Quality, "gpt-4").unwrap();
        assert_eq!(q.provider, "expensive");

        // Override with Cost
        let c = router.select_with_policy(Policy::Cost, "gpt-4").unwrap();
        assert_eq!(c.provider, "cheap");
    }

    #[test]
    fn test_set_and_get_policy() {
        let router = Router::new(RouterConfig::default());
        assert_eq!(router.get_policy(), Policy::Fallback);

        router.set_policy(Policy::Cost);
        assert_eq!(router.get_policy(), Policy::Cost);

        router.set_policy(Policy::Quality);
        assert_eq!(router.get_policy(), Policy::Quality);
    }

    #[test]
    fn test_set_aliases() {
        let router = Router::new(RouterConfig::default());
        let mut new_aliases = HashMap::new();
        new_aliases.insert("custom".to_string(), "my/model".to_string());

        router.set_aliases(new_aliases);
        assert_eq!(router.resolve_alias("custom"), "my/model");
        // Old aliases should be gone
        assert_eq!(router.resolve_alias("fast"), "fast"); // No longer aliased
    }

    #[test]
    fn test_metrics_no_samples() {
        let collector = MetricsCollector::new(100);
        let metrics = collector.get_metrics("nonexistent");
        assert_eq!(metrics.total_requests, 0);
        assert_eq!(metrics.avg_latency_ms, 0.0);
        assert_eq!(metrics.success_rate, 0.0);
    }

    #[test]
    fn test_metrics_avg_latency() {
        let collector = MetricsCollector::new(100);
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: 200,
            success: true,
            tokens_used: 200,
            cost: 0.02,
            timestamp: chrono::Utc::now(),
        });

        let metrics = collector.get_metrics("test");
        assert!((metrics.avg_latency_ms - 150.0).abs() < 0.01);
    }

    #[test]
    fn test_metrics_avg_cost_per_1k() {
        let collector = MetricsCollector::new(100);
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 1000,
            cost: 0.05,
            timestamp: chrono::Utc::now(),
        });

        let metrics = collector.get_metrics("test");
        assert!((metrics.avg_cost_per_1k - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_metrics_zero_tokens_cost() {
        let collector = MetricsCollector::new(100);
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 0,
            cost: 0.0,
            timestamp: chrono::Utc::now(),
        });

        let metrics = collector.get_metrics("test");
        assert_eq!(metrics.avg_cost_per_1k, 0.0);
    }

    #[test]
    fn test_candidate_serialization() {
        let c = Candidate {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        };
        let json = serde_json::to_string(&c).unwrap();
        let deserialized: Candidate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.provider, "openai");
        assert_eq!(deserialized.model, "gpt-4");
        assert!((deserialized.cost_per_1k - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn test_policy_serialization() {
        assert_eq!(serde_json::to_string(&Policy::Cost).unwrap(), "\"cost\"");
        assert_eq!(serde_json::to_string(&Policy::Quality).unwrap(), "\"quality\"");
        assert_eq!(serde_json::to_string(&Policy::Latency).unwrap(), "\"latency\"");
        assert_eq!(serde_json::to_string(&Policy::RoundRobin).unwrap(), "\"round_robin\"");
        assert_eq!(serde_json::to_string(&Policy::Fallback).unwrap(), "\"fallback\"");
    }

    #[test]
    fn test_policy_deserialization() {
        let p: Policy = serde_json::from_str("\"cost\"").unwrap();
        assert_eq!(p, Policy::Cost);

        let p: Policy = serde_json::from_str("\"fallback\"").unwrap();
        assert_eq!(p, Policy::Fallback);
    }

    #[test]
    fn test_router_register_and_use_provider() {
        struct MockProvider;
        #[async_trait::async_trait]
        impl LLMProvider for MockProvider {
            async fn chat(&self, _: &[Message], _: &[ToolDefinition], _: &str, _: &ChatOptions) -> Result<LLMResponse, FailoverError> {
                Ok(LLMResponse { content: "mock".into(), tool_calls: vec![], finish_reason: "stop".into(), usage: None })
            }
            fn default_model(&self) -> &str { "mock-model" }
            fn name(&self) -> &str { "mock" }
        }

        let router = Router::new(RouterConfig::default());
        router.register_provider("mock", Arc::new(MockProvider));
        router.add_candidate(Candidate {
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            cost_per_1k: 0.0,
            quality_score: 0.5,
            priority: 1,
        });

        let selected = router.select("mock-model");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().provider, "mock");
    }

    #[test]
    fn test_merge_aliases_does_not_modify_inputs() {
        let defaults = default_aliases();
        let mut custom = HashMap::new();
        custom.insert("custom".to_string(), "custom/model".to_string());

        let merged = merge_aliases(&defaults, &custom);
        assert_eq!(merged.len(), defaults.len() + 1);

        // Originals should not be modified
        assert!(!defaults.contains_key("custom"));
    }

    #[test]
    fn test_router_metrics_accessor() {
        let router = Router::new(RouterConfig::default());
        let metrics = router.metrics();
        metrics.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        let m = metrics.get_metrics("test");
        assert_eq!(m.total_requests, 1);
    }

    #[test]
    fn test_prune_no_samples() {
        let collector = MetricsCollector::new(100);
        // Should not panic with empty collector
        collector.prune(std::time::Duration::from_secs(3600));
    }
}

//! Provider resolution: resolves model references to provider configurations.
//!
//! Translated from Go `module/config/provider_resolver.go`.
//!
//! This module provides:
//! - [`resolve_model_config`] - resolves a model reference to a full provider config
//! - [`get_model_by_name`] - finds a model by name with round-robin load balancing
//! - [`get_effective_llm`] - gets the effective LLM for the default agent
//! - [`infer_provider_from_model`] - infers provider from model name
//! - [`infer_default_model`] - gets default model for a provider
//! - [`get_default_api_base`] - gets default API base URL for a provider

use serde::{Deserialize, Serialize};

use crate::{Config, ConfigError, ModelConfig, Result};

// ============================================================================
// Resolution types
// ============================================================================

/// Model resolution result with primary and fallback models.
/// Mirrors Go `ModelResolution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResolution {
    pub primary: String,
    pub fallbacks: Vec<String>,
}

/// Resolved provider and model configuration.
/// Mirrors Go `ProviderResolution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResolution {
    pub provider_name: String,
    pub model_name: String,
    pub api_key: String,
    pub api_base: String,
    pub proxy: String,
    pub auth_method: String,
    pub connect_mode: String,
    pub workspace: String,
    pub enabled: bool,
}

impl Default for ProviderResolution {
    fn default() -> Self {
        Self {
            provider_name: String::new(),
            model_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
            proxy: String::new(),
            auth_method: String::new(),
            connect_mode: String::new(),
            workspace: String::new(),
            enabled: true,
        }
    }
}

// ============================================================================
// Core resolution functions
// ============================================================================

/// Resolve a model reference from the config's model list.
///
/// `model_ref` can be either a `model_name` or a `vendor/model` string.
/// Returns `ProviderResolution` with all configuration needed for API calls.
///
/// Mirrors Go `ResolveModelConfig`.
pub fn resolve_model_config(cfg: &Config, model_ref: &str) -> Result<ProviderResolution> {
    let model_ref = model_ref.trim();
    if model_ref.is_empty() {
        return Err(ConfigError::Validation("model reference is empty".into()));
    }

    // First, try to find by model_name (exact match)
    for mc in &cfg.model_list {
        if mc.model_name == model_ref {
            return Ok(resolve_from_model_config(mc));
        }
    }

    // Then, try to find by model field (vendor/model format)
    if model_ref.contains('/') {
        for mc in &cfg.model_list {
            if mc.model == model_ref {
                return Ok(resolve_from_model_config(mc));
            }
        }
    }

    // Not found, try to infer provider from model name
    let inferred = infer_provider_from_model(model_ref);
    if !inferred.is_empty() {
        return Ok(ProviderResolution {
            provider_name: inferred.clone(),
            model_name: model_ref.to_string(),
            api_base: get_default_api_base(&inferred),
            enabled: true,
            ..Default::default()
        });
    }

    Err(ConfigError::Validation(format!(
        "model {:?} not found in model_list",
        model_ref
    )))
}

/// Convert a ModelConfig to ProviderResolution.
/// Mirrors Go `resolveFromModelConfig`.
fn resolve_from_model_config(mc: &ModelConfig) -> ProviderResolution {
    let (provider_name, model_name) = if mc.model.contains('/') {
        let mut parts = mc.model.splitn(2, '/');
        let provider = parts.next().unwrap_or("").to_lowercase();
        let model = parts.next().unwrap_or("").to_string();
        (provider, model)
    } else {
        let provider = infer_provider_from_model(&mc.model);
        (provider, mc.model.clone())
    };

    let api_base = if mc.api_base.is_empty() {
        get_default_api_base(&provider_name)
    } else {
        mc.api_base.clone()
    };

    ProviderResolution {
        provider_name,
        model_name,
        api_key: mc.api_key.clone(),
        api_base,
        proxy: mc.proxy.clone(),
        auth_method: mc.auth_method.clone(),
        connect_mode: mc.connect_mode.clone(),
        workspace: mc.workspace.clone(),
        enabled: true,
    }
}

/// Find a model configuration by name or model field (returns reference).
/// For round-robin load balancing, use `get_model_by_name` instead.
pub fn find_model_by_name<'a>(cfg: &'a Config, model_ref: &str) -> Result<&'a ModelConfig> {
    // Search by model_name
    for mc in &cfg.model_list {
        if mc.model_name == model_ref {
            return Ok(mc);
        }
    }

    // Search by model field
    for mc in &cfg.model_list {
        if mc.model == model_ref {
            return Ok(mc);
        }
    }

    Err(ConfigError::Validation(format!(
        "model {:?} not found in model_list",
        model_ref
    )))
}

/// Get the effective LLM reference for the default agent.
///
/// After migration, only the LLM field is used (old Provider/Model fields are removed).
/// Mirrors Go `GetEffectiveLLM`.
pub fn get_effective_llm(cfg: Option<&Config>) -> String {
    match cfg {
        Some(c) if !c.agents.defaults.llm.is_empty() => c.agents.defaults.llm.clone(),
        _ => "zhipu/glm-4.7-flash".to_string(),
    }
}

/// Infer the provider name from a model string.
///
/// Examines the model name for known keywords (e.g., "claude" -> "anthropic").
/// Mirrors Go `inferProviderFromModel`.
pub fn infer_provider_from_model(model: &str) -> String {
    let m = model.to_lowercase();
    if m.contains("claude") {
        return "anthropic".to_string();
    }
    if m.contains("gpt") {
        return "openai".to_string();
    }
    if m.contains("gemini") {
        return "gemini".to_string();
    }
    if m.contains("glm") || m.contains("zhipu") {
        return "zhipu".to_string();
    }
    if m.contains("groq") {
        return "groq".to_string();
    }
    if m.contains("llama") {
        return "ollama".to_string();
    }
    if m.contains("moonshot") || m.contains("kimi") {
        return "moonshot".to_string();
    }
    if m.contains("nvidia") {
        return "nvidia".to_string();
    }
    if m.contains("deepseek") {
        return "deepseek".to_string();
    }
    if m.contains("mistral") || m.contains("mixtral") || m.contains("codestral") {
        return "mistral".to_string();
    }
    if m.contains("command") || m.contains("cohere") {
        return "cohere".to_string();
    }
    if m.contains("sonar") || m.contains("perplexity") {
        return "perplexity".to_string();
    }
    String::new()
}

/// Get the default API base URL for a provider.
///
/// Mirrors Go `getDefaultAPIBase`.
pub fn get_default_api_base(provider: &str) -> String {
    match provider {
        "anthropic" | "claude" => "https://api.anthropic.com/v1".to_string(),
        "openai" | "gpt" => "https://api.openai.com/v1".to_string(),
        "openrouter" => "https://openrouter.ai/api/v1".to_string(),
        "groq" => "https://api.groq.com/openai/v1".to_string(),
        "zhipu" | "glm" => "https://open.bigmodel.cn/api/paas/v4".to_string(),
        "gemini" | "google" => "https://generativelanguage.googleapis.com/v1beta".to_string(),
        "nvidia" => "https://integrate.api.nvidia.com/v1".to_string(),
        "ollama" => "http://localhost:11434/v1".to_string(),
        "moonshot" | "kimi" => "https://api.moonshot.cn/v1".to_string(),
        "deepseek" => "https://api.deepseek.com/v1".to_string(),
        "mistral" => "https://api.mistral.ai/v1".to_string(),
        "cohere" => "https://api.cohere.ai/v2".to_string(),
        "perplexity" => "https://api.perplexity.ai/v1".to_string(),
        "together" => "https://api.together.xyz/v1".to_string(),
        "fireworks" => "https://api.fireworks.ai/inference/v1".to_string(),
        "cerebras" => "https://api.cerebras.ai/v1".to_string(),
        "sambanova" => "https://api.sambanova.ai/v1".to_string(),
        "shengsuanyun" => "https://router.shengsuanyun.com/api/v1".to_string(),
        "github_copilot" => "localhost:4321".to_string(),
        _ => String::new(),
    }
}

/// Return the default model for a given provider name.
///
/// When a provider is known but no specific model is configured, this provides
/// a reasonable default. Mirrors Go `inferDefaultModel`.
pub fn infer_default_model(provider: &str) -> String {
    match provider {
        "anthropic" | "claude" => "claude-sonnet-4-20250514".to_string(),
        "openai" | "gpt" => "gpt-4o".to_string(),
        "zhipu" | "glm" => "glm-4.7-flash".to_string(),
        "groq" => "llama-3.3-70b-versatile".to_string(),
        "ollama" => "llama3.3".to_string(),
        "gemini" | "google" => "gemini-2.0-flash-exp".to_string(),
        "nvidia" => "nvidia/llama-3.1-nemotron-70b-instruct".to_string(),
        "moonshot" | "kimi" => "moonshot-v1-8k".to_string(),
        "deepseek" => "deepseek-chat".to_string(),
        "mistral" => "mistral-large-latest".to_string(),
        "cohere" => "command-r-plus".to_string(),
        "perplexity" => "sonar".to_string(),
        "together" => "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
        "fireworks" => "accounts/fireworks/models/llama-v3p3-70b-instruct".to_string(),
        "cerebras" => "llama-3.3-70b".to_string(),
        "sambanova" => "Meta-Llama-3.3-70B-Instruct".to_string(),
        _ => String::new(),
    }
}

/// Find a model configuration by name with round-robin load balancing.
///
/// When multiple models match, uses an atomic counter to distribute load.
/// Mirrors Go `GetModelByName`.
pub fn get_model_by_name(cfg: &Config, model_ref: &str) -> Result<ModelConfig> {
    let mut matches: Vec<&ModelConfig> = Vec::new();

    for mc in &cfg.model_list {
        if mc.model_name == model_ref {
            matches.push(mc);
        }
    }

    if matches.is_empty() {
        for mc in &cfg.model_list {
            if mc.model == model_ref {
                matches.push(mc);
            }
        }
    }

    if matches.is_empty() {
        return Err(ConfigError::Validation(format!(
            "model {:?} not found in model_list",
            model_ref
        )));
    }

    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }

    static RR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let idx =
        RR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % (matches.len() as u64);
    Ok(matches[idx as usize].clone())
}

/// Resolve the model resolution (primary + fallbacks) for the default agent.
///
/// Mirrors Go `ModelResolution` construction.
pub fn resolve_model_resolution(cfg: &Config) -> ModelResolution {
    let llm = get_effective_llm(Some(cfg));
    ModelResolution {
        primary: llm,
        fallbacks: vec![],
    }
}

// ============================================================================
// ProviderResolver struct (convenience wrapper)
// ============================================================================

/// Provider resolver: finds model config by model name/alias.
pub struct ProviderResolver;

impl ProviderResolver {
    /// Find a model config by name from the model list.
    pub fn find_by_name<'a>(models: &'a [ModelConfig], name: &str) -> Option<&'a ModelConfig> {
        models.iter().find(|m| m.model_name == name)
    }

    /// Find default model (first model in the list, or one marked as default).
    pub fn find_default<'a>(models: &'a [ModelConfig]) -> Option<&'a ModelConfig> {
        models.first()
    }

    /// Resolve model string to a provider and model identifier.
    pub fn resolve_model_string(model_str: &str) -> (&str, &str) {
        if let Some(slash_pos) = model_str.find('/') {
            (&model_str[..slash_pos], &model_str[slash_pos + 1..])
        } else {
            ("openai", model_str)
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests;

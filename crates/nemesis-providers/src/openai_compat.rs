//! OpenAI-compatible provider adapter.
//!
//! Implements a provider that works with any OpenAI-compatible API endpoint,
//! including providers like Groq, DeepSeek, Zhipu, Moonshot, OpenRouter, etc.
//!
//! Handles:
//! - Chat completion API calls (`/chat/completions`)
//! - Model name normalization (strips provider prefix where appropriate)
//! - Special handling for models that use `max_completion_tokens` vs `max_tokens`
//! - Tool call parsing from OpenAI-format responses

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenAI-compatible provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAICompatConfig {
    /// Provider display name.
    pub name: String,
    /// API base URL (e.g., "https://api.openai.com/v1").
    pub base_url: String,
    /// API key for authentication.
    pub api_key: String,
    /// Default model to use if none specified.
    #[serde(default)]
    pub default_model: String,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Optional HTTP proxy URL.
    #[serde(default)]
    pub proxy: Option<String>,
}

fn default_timeout() -> u64 {
    600 // 10 minutes (matches Go implementation)
}

impl Default for OpenAICompatConfig {
    fn default() -> Self {
        Self {
            name: "openai-compat".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            default_model: String::new(),
            timeout_secs: 600,
            proxy: None,
        }
    }
}

/// OpenAI-compatible API provider.
///
/// Works with any API that follows the OpenAI chat completions format,
/// including but not limited to: OpenAI, Groq, DeepSeek, Zhipu, Moonshot,
/// OpenRouter, Together, Fireworks, etc.
pub struct OpenAICompatProvider {
    config: OpenAICompatConfig,
    client: reqwest::Client,
}

impl OpenAICompatProvider {
    /// Create a new OpenAI-compatible provider.
    pub fn new(config: OpenAICompatConfig) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs));

        if let Some(ref proxy_url) = config.proxy {
            if let Ok(proxy) = reqwest::Proxy::all(proxy_url) {
                builder = builder.proxy(proxy);
            }
        }

        let client = builder.build().expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Normalize model using the instance's base URL.
    fn normalize_model_with_base(&self, model: &str) -> String {
        let idx = match model.find('/') {
            Some(i) => i,
            None => return model.to_string(),
        };

        let base_lower = self.config.base_url.to_lowercase();
        // OpenRouter keeps the full path.
        if base_lower.contains("openrouter.ai") {
            return model.to_string();
        }

        let prefix = &model[..idx];
        let prefix_lower = prefix.to_lowercase();
        match prefix_lower.as_str() {
            "moonshot" | "nvidia" | "groq" | "ollama" | "deepseek" | "google"
            | "openrouter" | "zhipu" => model[idx + 1..].to_string(),
            _ => model.to_string(),
        }
    }

    /// Check if model uses `max_completion_tokens` instead of `max_tokens`.
    fn uses_completion_tokens(model: &str) -> bool {
        let lower = model.to_lowercase();
        lower.contains("glm") || lower.contains("o1") || lower.contains("gpt-5")
    }

    /// Check if model requires temperature=1 (Kimi k2).
    fn requires_fixed_temperature(model: &str) -> bool {
        let lower = model.to_lowercase();
        lower.contains("kimi") && lower.contains("k2")
    }

    /// Build the request body for the OpenAI-compatible chat completions API.
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> serde_json::Value {
        let normalized_model = self.normalize_model_with_base(model);

        let mut body = serde_json::json!({
            "model": normalized_model,
            "messages": messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        // Max tokens handling.
        if let Some(max_tokens) = options.max_tokens {
            if Self::uses_completion_tokens(&normalized_model) {
                body["max_completion_tokens"] = serde_json::json!(max_tokens);
            } else {
                body["max_tokens"] = serde_json::json!(max_tokens);
            }
        }

        // Temperature handling.
        if let Some(temp) = options.temperature {
            if Self::requires_fixed_temperature(&normalized_model) {
                body["temperature"] = serde_json::json!(1.0);
            } else {
                body["temperature"] = serde_json::json!(temp);
            }
        }

        if let Some(top_p) = options.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }

        if let Some(stop) = &options.stop {
            body["stop"] = serde_json::json!(stop);
        }

        body
    }
}

/// Parse the OpenAI-format response body.
fn parse_response(data: &serde_json::Value) -> LLMResponse {
    let choices = data.get("choices").and_then(|c| c.as_array());

    let content = choices
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let finish_reason = choices
        .and_then(|c| c.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .unwrap_or("stop")
        .to_string();

    let usage = data.get("usage").map(|u| {
        // Extract cached tokens from provider-specific fields:
        // - DeepSeek: prompt_cache_hit_tokens
        // - OpenAI: prompt_tokens_details.cached_tokens
        let cached = u.get("prompt_cache_hit_tokens")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                u.get("prompt_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(|v| v.as_i64())
            });
        UsageInfo {
            prompt_tokens: u["prompt_tokens"].as_i64().unwrap_or(0),
            completion_tokens: u["completion_tokens"].as_i64().unwrap_or(0),
            total_tokens: u["total_tokens"].as_i64().unwrap_or(0),
            cached_tokens: cached,
            cache_creation_tokens: None,
            cache_read_tokens: cached,
        }
    });

    let tool_calls = choices
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("tool_calls"))
        .and_then(|tc| tc.as_array())
        .map(|tc_array| {
            tc_array
                .iter()
                .filter_map(|tc| {
                    let id = tc["id"].as_str()?.to_string();
                    let func = tc.get("function")?;
                    let name = func["name"].as_str()?.to_string();
                    let arguments_str = func["arguments"].as_str().unwrap_or("{}").to_string();

                    // Parse arguments from JSON string
                    let arguments: HashMap<String, serde_json::Value> =
                        serde_json::from_str(&arguments_str).unwrap_or_else(|_| {
                            let mut m = HashMap::new();
                            m.insert("raw".to_string(), serde_json::Value::String(arguments_str.clone()));
                            m
                        });

                    Some(ToolCall {
                        id,
                        call_type: Some("function".to_string()),
                        function: Some(FunctionCall {
                            name,
                            arguments: arguments_str,
                        }),
                        name: None,
                        arguments: Some(arguments),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Extract reasoning_content from thinking-mode models (DeepSeek R1, GLM, etc.)
    let reasoning_content = choices
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("reasoning_content"))
        .and_then(|rc| rc.as_str())
        .map(|s| s.to_string());

    LLMResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
        reasoning_content,
        extra: HashMap::new(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        if self.config.base_url.is_empty() {
            return Err(FailoverError::Format {
                provider: self.config.name.clone(),
                message: "API base URL not configured".to_string(),
            });
        }

        let model = if model.is_empty() {
            &self.config.default_model
        } else {
            model
        };

        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let body = self.build_request_body(messages, tools, model, options);

        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if !self.config.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.config.api_key));
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|_| FailoverError::Timeout {
                provider: self.config.name.clone(),
                model: model.to_string(),
            })?;

        let status = resp.status().as_u16();

        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            return Err(FailoverError::from_status(
                &self.config.name,
                model,
                status,
                &text,
            ));
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| FailoverError::Format {
            provider: self.config.name.clone(),
            message: e.to_string(),
        })?;

        Ok(parse_response(&data))
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        &self.config.name
    }
}

/// Normalize model name (static utility).
///
/// Strips the provider prefix for known providers.
/// Preserves the prefix for OpenRouter.
pub fn normalize_model(model: &str, api_base: &str) -> String {
    let idx = match model.find('/') {
        Some(i) => i,
        None => return model.to_string(),
    };

    if api_base.to_lowercase().contains("openrouter.ai") {
        return model.to_string();
    }

    let prefix = &model[..idx];
    let prefix_lower = prefix.to_lowercase();
    match prefix_lower.as_str() {
        "moonshot" | "nvidia" | "groq" | "ollama" | "deepseek" | "google" | "openrouter"
        | "zhipu" => model[idx + 1..].to_string(),
        _ => model.to_string(),
    }
}

#[cfg(test)]
mod tests;

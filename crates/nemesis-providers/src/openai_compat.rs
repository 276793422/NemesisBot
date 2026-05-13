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

    let usage = data.get("usage").map(|u| UsageInfo {
        prompt_tokens: u["prompt_tokens"].as_i64().unwrap_or(0),
        completion_tokens: u["completion_tokens"].as_i64().unwrap_or(0),
        total_tokens: u["total_tokens"].as_i64().unwrap_or(0),
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

    LLMResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
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
mod tests {
    use super::*;

    #[test]
    fn test_normalize_model_strips_known_prefix() {
        assert_eq!(normalize_model("deepseek/chat", "https://api.deepseek.com"), "chat");
        assert_eq!(normalize_model("groq/llama3", "https://api.groq.com"), "llama3");
        assert_eq!(normalize_model("zhipu/glm-4", "https://open.bigmodel.cn"), "glm-4");
        assert_eq!(normalize_model("ollama/llama3", "http://localhost:11434"), "llama3");
    }

    #[test]
    fn test_normalize_model_preserves_openrouter() {
        assert_eq!(
            normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"),
            "openai/gpt-4"
        );
    }

    #[test]
    fn test_normalize_model_no_prefix() {
        assert_eq!(normalize_model("gpt-4", "https://api.openai.com"), "gpt-4");
    }

    #[test]
    fn test_normalize_model_unknown_prefix() {
        assert_eq!(normalize_model("myprovider/model", "https://example.com"), "myprovider/model");
    }

    #[test]
    fn test_uses_completion_tokens() {
        assert!(OpenAICompatProvider::uses_completion_tokens("glm-4"));
        assert!(OpenAICompatProvider::uses_completion_tokens("o1-preview"));
        assert!(OpenAICompatProvider::uses_completion_tokens("gpt-5"));
        assert!(!OpenAICompatProvider::uses_completion_tokens("gpt-4"));
        assert!(!OpenAICompatProvider::uses_completion_tokens("deepseek-chat"));
    }

    #[test]
    fn test_requires_fixed_temperature() {
        assert!(OpenAICompatProvider::requires_fixed_temperature("kimi-k2"));
        assert!(OpenAICompatProvider::requires_fixed_temperature("Kimi K2"));
        assert!(!OpenAICompatProvider::requires_fixed_temperature("kimi-v1"));
        assert!(!OpenAICompatProvider::requires_fixed_temperature("gpt-4"));
    }

    #[test]
    fn test_parse_response_simple() {
        let data = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "Hello!",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.finish_reason, "stop");
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_response_with_tool_calls() {
        let data = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/tmp/test\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = parse_response(&data);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_123");
        assert_eq!(
            resp.tool_calls[0].function.as_ref().unwrap().name,
            "read_file"
        );
    }

    #[test]
    fn test_parse_response_empty_choices() {
        let data = serde_json::json!({
            "choices": []
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "");
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn test_build_request_body_basic() {
        let config = OpenAICompatConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);

        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];

        let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
        assert_eq!(body["model"], "gpt-4");
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let config = OpenAICompatConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);

        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];

        let body = provider.build_request_body(
            &[],
            &tools,
            "gpt-4",
            &ChatOptions {
                temperature: Some(0.7),
                max_tokens: Some(1000),
                ..Default::default()
            },
        );
        assert!(body.get("tools").is_some());
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 1000);
    }

    #[test]
    fn test_config_default() {
        let config = OpenAICompatConfig::default();
        assert_eq!(config.name, "openai-compat");
        assert_eq!(config.timeout_secs, 600);
        assert!(config.base_url.is_empty());
        assert!(config.api_key.is_empty());
        assert!(config.default_model.is_empty());
        assert!(config.proxy.is_none());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_normalize_model_with_base() {
        assert_eq!(normalize_model("nvidia/llama3", "https://api.nvidia.com"), "llama3");
        assert_eq!(normalize_model("ollama/llama3", "http://localhost:11434"), "llama3");
        assert_eq!(normalize_model("google/gemini", "https://generativelanguage.googleapis.com"), "gemini");
        assert_eq!(normalize_model("moonshot/kimi", "https://api.moonshot.cn"), "kimi");
    }

    #[test]
    fn test_normalize_model_unknown_provider_prefix() {
        assert_eq!(normalize_model("myco/model", "https://example.com"), "myco/model");
    }

    #[test]
    fn test_normalize_model_openrouter_preserves() {
        assert_eq!(
            normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"),
            "openai/gpt-4"
        );
        assert_eq!(
            normalize_model("anthropic/claude-3", "https://OPENROUTER.AI/api/v1"),
            "anthropic/claude-3" // case-insensitive
        );
    }

    #[test]
    fn test_parse_response_no_usage() {
        let data = serde_json::json!({
            "choices": [{
                "message": { "content": "No usage info", "role": "assistant" },
                "finish_reason": "stop"
            }]
        });
        let resp = parse_response(&data);
        assert!(resp.usage.is_none());
        assert_eq!(resp.content, "No usage info");
    }

    #[test]
    fn test_parse_response_null_content() {
        let data = serde_json::json!({
            "choices": [{
                "message": { "content": null, "role": "assistant" },
                "finish_reason": "stop"
            }]
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_response_null_finish_reason() {
        let data = serde_json::json!({
            "choices": [{
                "message": { "content": "test", "role": "assistant" },
                "finish_reason": null
            }]
        });
        let resp = parse_response(&data);
        assert_eq!(resp.finish_reason, "stop"); // defaults to "stop"
    }

    #[test]
    fn test_parse_response_no_choices_field() {
        let data = serde_json::json!({});
        let resp = parse_response(&data);
        assert_eq!(resp.content, "");
        assert_eq!(resp.finish_reason, "stop");
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_response_multiple_tool_calls() {
        let data = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "read_file", "arguments": "{\"path\":\"/a\"}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "write_file", "arguments": "{\"path\":\"/b\",\"content\":\"hello\"}" }
                        },
                        {
                            "id": "call_3",
                            "type": "function",
                            "function": { "name": "run_command", "arguments": "{\"cmd\":\"ls\"}" }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = parse_response(&data);
        assert_eq!(resp.tool_calls.len(), 3);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[1].id, "call_2");
        assert_eq!(resp.tool_calls[2].id, "call_3");
    }

    #[test]
    fn test_parse_response_tool_call_with_invalid_args() {
        let data = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call_bad",
                        "type": "function",
                        "function": { "name": "test", "arguments": "not valid json" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = parse_response(&data);
        assert_eq!(resp.tool_calls.len(), 1);
        // Arguments should be parsed as raw fallback
        assert!(resp.tool_calls[0].arguments.is_some());
        assert!(resp.tool_calls[0].arguments.as_ref().unwrap().contains_key("raw"));
    }

    #[test]
    fn test_parse_response_tool_call_missing_id() {
        let data = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "type": "function",
                        "function": { "name": "test", "arguments": "{}" }
                    }]
                }
            }]
        });
        let resp = parse_response(&data);
        // Missing id should be filtered out by filter_map
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_uses_completion_tokens_additional_models() {
        assert!(OpenAICompatProvider::uses_completion_tokens("glm-4-plus"));
        assert!(OpenAICompatProvider::uses_completion_tokens("glm-4-flash"));
        assert!(OpenAICompatProvider::uses_completion_tokens("o1"));
        assert!(!OpenAICompatProvider::uses_completion_tokens("gpt-4-turbo"));
        assert!(!OpenAICompatProvider::uses_completion_tokens("claude-3-opus"));
    }

    #[test]
    fn test_requires_fixed_temperature_additional() {
        assert!(OpenAICompatProvider::requires_fixed_temperature("kimi-k2-latest"));
        assert!(OpenAICompatProvider::requires_fixed_temperature("Kimi-K2-Pro"));
        assert!(!OpenAICompatProvider::requires_fixed_temperature("kimi-v1"));
        assert!(!OpenAICompatProvider::requires_fixed_temperature("gpt-4"));
    }

    #[test]
    fn test_build_request_body_completion_tokens() {
        let config = OpenAICompatConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);

        let body = provider.build_request_body(
            &[], &[], "glm-4",
            &ChatOptions { max_tokens: Some(2048), ..Default::default() }
        );
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn test_build_request_body_kimi_fixed_temperature() {
        let config = OpenAICompatConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);

        // Kimi K2 should force temperature=1.0
        let body = provider.build_request_body(
            &[], &[], "kimi-k2",
            &ChatOptions { temperature: Some(0.5), ..Default::default() }
        );
        assert_eq!(body["temperature"], 1.0);
    }

    #[test]
    fn test_build_request_body_no_tools_no_tool_choice() {
        let config = OpenAICompatConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);

        let body = provider.build_request_body(&[], &[], "gpt-4", &ChatOptions::default());
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn test_build_request_body_with_stop_and_top_p() {
        let config = OpenAICompatConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);

        let body = provider.build_request_body(
            &[], &[], "gpt-4",
            &ChatOptions {
                top_p: Some(0.95),
                stop: Some(vec!["END".to_string()]),
                ..Default::default()
            }
        );
        assert_eq!(body["top_p"], 0.95);
        assert!(body.get("stop").is_some());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = OpenAICompatConfig {
            name: "my-compat".to_string(),
            base_url: "https://api.test.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            default_model: "test-model".to_string(),
            timeout_secs: 120,
            proxy: Some("http://proxy:8080".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OpenAICompatConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "my-compat");
        assert_eq!(deserialized.base_url, "https://api.test.com/v1");
        assert_eq!(deserialized.proxy, Some("http://proxy:8080".to_string()));
    }

    #[test]
    fn test_default_model_and_name_accessors() {
        let config = OpenAICompatConfig {
            name: "my-provider".to_string(),
            base_url: "https://api.test.com/v1".to_string(),
            api_key: "test".to_string(),
            default_model: "default-model".to_string(),
            timeout_secs: 30,
            proxy: None,
        };
        let provider = OpenAICompatProvider::new(config);
        assert_eq!(provider.default_model(), "default-model");
        assert_eq!(provider.name(), "my-provider");
    }
}

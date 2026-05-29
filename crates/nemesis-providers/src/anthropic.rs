//! Anthropic/Claude provider (Anthropic Messages API).

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";

/// Anthropic provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicConfig {
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    120
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            timeout_secs: 120,
        }
    }
}

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: reqwest::Client,
    token_source: Option<Box<dyn Fn() -> Result<String, String> + Send + Sync>>,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build HTTP client");
        Self {
            config,
            client,
            token_source: None,
        }
    }

    /// Create with a token source for OAuth-style token refresh.
    pub fn with_token_source(
        config: AnthropicConfig,
        token_source: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build HTTP client");
        Self {
            config,
            client,
            token_source: Some(token_source),
        }
    }

    /// Create with a token source and a custom base URL.
    /// Equivalent to Go's NewProviderWithTokenSourceAndBaseURL.
    pub fn with_token_source_and_base_url(
        mut config: AnthropicConfig,
        token_source: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
        base_url: &str,
    ) -> Self {
        if !base_url.is_empty() {
            config.base_url = normalize_base_url(base_url);
        }
        Self::with_token_source(config, token_source)
    }

    /// Get the configured base URL.
    /// Equivalent to Go's Provider.BaseURL().
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Get the API key, potentially refreshing via token source.
    fn get_api_key(&self) -> Result<String, FailoverError> {
        if let Some(ref ts) = self.token_source {
            ts().map_err(|_| FailoverError::Auth {
                provider: self.config.default_model.clone(),
                model: self.config.default_model.clone(),
                status: 0,
            })
        } else {
            Ok(self.config.api_key.clone())
        }
    }

    /// Build the Anthropic Messages API request body.
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> serde_json::Value {
        let mut system_parts = Vec::new();
        let mut api_messages = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    system_parts.push(serde_json::json!({
                        "type": "text",
                        "text": msg.content
                    }));
                }
                "user" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        // Tool result
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tc_id,
                                "content": msg.content
                            }]
                        }));
                    } else {
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": msg.content
                        }));
                    }
                }
                "assistant" => {
                    if !msg.tool_calls.is_empty() {
                        let mut content: Vec<serde_json::Value> = Vec::new();
                        if !msg.content.is_empty() {
                            content.push(serde_json::json!({
                                "type": "text",
                                "text": msg.content
                            }));
                        }
                        for tc in &msg.tool_calls {
                            let name = tc.name.as_deref()
                                .or_else(|| tc.function.as_ref().map(|f| f.name.as_str()))
                                .unwrap_or("");
                            let input = tc.arguments.as_ref()
                                .map(|args| serde_json::Value::Object(args.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
                                .unwrap_or(serde_json::json!({}));
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": name,
                                "input": input
                            }));
                        }
                        api_messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content
                        }));
                    } else {
                        api_messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content
                        }));
                    }
                }
                "tool" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tc_id,
                                "content": msg.content
                            }]
                        }));
                    }
                }
                _ => {}
            }
        }

        let max_tokens = options.max_tokens.unwrap_or(4096);

        let mut body = serde_json::json!({
            "model": model,
            "messages": api_messages,
            "max_tokens": max_tokens,
        });

        if !system_parts.is_empty() {
            body["system"] = serde_json::json!(system_parts);
        }

        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(translate_tools(tools));
        }

        body
    }
}

/// Translate tool definitions to Anthropic format.
fn translate_tools(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .filter(|t| t.tool_type == "function")
        .map(|t| {
            let mut tool = serde_json::json!({
                "name": t.function.name,
                "input_schema": {
                    "type": "object",
                    "properties": t.function.parameters.get("properties").unwrap_or(&serde_json::json!({})),
                }
            });
            if !t.function.description.is_empty() {
                tool["description"] = serde_json::json!(t.function.description);
            }
            if let Some(req) = t.function.parameters.get("required").and_then(|r| r.as_array()) {
                let req_strs: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
                tool["input_schema"]["required"] = serde_json::json!(req_strs);
            }
            tool
        })
        .collect()
}

/// Parse the Anthropic Messages API response.
fn parse_response(data: &serde_json::Value) -> LLMResponse {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(blocks) = data.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        content.push_str(text);
                    }
                }
                "tool_use" => {
                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));

                    let arguments: HashMap<String, serde_json::Value> =
                        serde_json::from_value(input.clone()).unwrap_or_else(|_| {
                            let mut m = HashMap::new();
                            m.insert("raw".to_string(), input.clone());
                            m
                        });

                    tool_calls.push(ToolCall {
                        id,
                        call_type: Some("tool_use".to_string()),
                        function: Some(FunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(&input).unwrap_or_default(),
                        }),
                        name: Some(name),
                        arguments: Some(arguments),
                    });
                }
                _ => {}
            }
        }
    }

    let finish_reason = match data
        .get("stop_reason")
        .and_then(|r| r.as_str())
        .unwrap_or("stop")
    {
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "end_turn" | _ => "stop",
    };

    let usage = if let Some(u) = data.get("usage") {
        let prompt = u.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let completion = u.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let cache_creation = u.get("cache_creation_input_tokens").and_then(|v| v.as_i64());
        let cache_read = u.get("cache_read_input_tokens").and_then(|v| v.as_i64());
        Some(UsageInfo {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            // cached_tokens is intentionally None here; Anthropic uses cache_read_tokens
            // instead. The fallback chain in loop_executor.rs uses .or(cached_tokens).
            cached_tokens: None,
            cache_creation_tokens: cache_creation,
            cache_read_tokens: cache_read,
        })
    } else {
        None
    };

    LLMResponse {
        content,
        tool_calls,
        finish_reason: finish_reason.to_string(),
        usage,
        reasoning_content: None,
        extra: HashMap::new(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// Normalize the Anthropic base URL (strip trailing `/v1`).
pub fn normalize_base_url(url: &str) -> String {
    let base = url.trim().trim_end_matches('/');
    if base.is_empty() {
        return DEFAULT_BASE_URL.to_string();
    }
    let base = base.strip_suffix("/v1").unwrap_or(base);
    if base.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        base.to_string()
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let model = if model.is_empty() {
            &self.config.default_model
        } else {
            model
        };

        let api_key = self.get_api_key()?;

        let url = format!(
            "{}/v1/messages",
            self.config.base_url.trim_end_matches('/')
        );
        let body = self.build_request_body(messages, tools, model, options);

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|_| FailoverError::Timeout {
                provider: "anthropic".to_string(),
                model: model.to_string(),
            })?;

        let status = resp.status().as_u16();

        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            return Err(FailoverError::from_status("anthropic", model, status, &text));
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| FailoverError::Format {
            provider: "anthropic".to_string(),
            message: e.to_string(),
        })?;

        Ok(parse_response(&data))
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

#[cfg(test)]
mod tests;

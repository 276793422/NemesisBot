//! HTTP-based LLM provider (OpenAI-compatible).

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// HTTP provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpProviderConfig {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Optional HTTP proxy URL (e.g. "http://proxy:8080", "socks5://proxy:1080").
    #[serde(default)]
    pub proxy: Option<String>,
    /// If true, preserve the provider prefix in model names (e.g. "openrouter/model").
    #[serde(default)]
    pub preserve_prefix: bool,
}

fn default_timeout() -> u64 {
    120
}

/// OpenAI-compatible HTTP LLM provider.
pub struct HttpProvider {
    config: HttpProviderConfig,
    client: reqwest::Client,
}

impl HttpProvider {
    pub fn new(config: HttpProviderConfig) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs));

        // Configure HTTP proxy if specified.
        if let Some(ref proxy_url) = config.proxy {
            match reqwest::Proxy::all(proxy_url) {
                Ok(proxy) => {
                    builder = builder.proxy(proxy);
                    tracing::info!(
                        provider = %config.name,
                        proxy = %proxy_url,
                        "HTTP provider using proxy"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        provider = %config.name,
                        proxy = %proxy_url,
                        error = %e,
                        "Failed to configure proxy, using direct connection"
                    );
                }
            }
        }

        let client = builder.build().expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Normalize model name for API compatibility.
    ///
    /// Handles common provider-specific model naming conventions:
    /// - Strips "provider/" prefix (e.g. "openai/gpt-4" → "gpt-4")
    /// - Maps aliases (e.g. "gpt4" → "gpt-4")
    pub fn normalize_model(model: &str) -> String {
        let model = model.trim();

        // Strip provider prefix if present (e.g. "openai/gpt-4" → "gpt-4")
        let model = if let Some(slash_pos) = model.find('/') {
            &model[slash_pos + 1..]
        } else {
            model
        };

        // Common alias normalization
        match model {
            "gpt4" => "gpt-4".to_string(),
            "gpt4o" => "gpt-4o".to_string(),
            "gpt4-turbo" => "gpt-4-turbo".to_string(),
            "gpt35-turbo" => "gpt-3.5-turbo".to_string(),
            "claude3" => "claude-3-sonnet-20240229".to_string(),
            "claude3-opus" => "claude-3-opus-20240229".to_string(),
            "claude3-sonnet" => "claude-3-sonnet-20240229".to_string(),
            "claude3-haiku" => "claude-3-haiku-20240307".to_string(),
            _ => model.to_string(),
        }
    }

    /// Check if model uses max_completion_tokens instead of max_tokens.
    fn uses_completion_tokens(model: &str) -> bool {
        let lower = model.to_lowercase();
        lower.starts_with("o1-")
            || lower.starts_with("o3-")
            || lower.starts_with("gpt-5")
            || lower.contains("glm-")
    }

    /// Build request body for OpenAI-compatible API.
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> serde_json::Value {
        let normalized = if self.config.preserve_prefix {
            model.trim().to_string()
        } else {
            Self::normalize_model(model)
        };

        let mut body = serde_json::json!({
            "model": normalized,
            "messages": messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            // When tools are provided, set tool_choice to "auto" by default.
            body["tool_choice"] = serde_json::json!("auto");
        }

        // Temperature handling.
        if let Some(temp) = options.temperature {
            let lower = normalized.to_lowercase();
            // Some models don't support temperature (e.g. o1, o3)
            if !lower.starts_with("o1-") && !lower.starts_with("o3-") {
                body["temperature"] = serde_json::json!(temp);
            }
        } else {
            // Kimi k2 models require temperature = 1.0
            let lower = normalized.to_lowercase();
            if lower.contains("kimi") || lower.contains("moonshot") {
                body["temperature"] = serde_json::json!(1.0);
            }
        }

        if let Some(max_tokens) = options.max_tokens {
            if Self::uses_completion_tokens(&normalized) {
                body["max_completion_tokens"] = serde_json::json!(max_tokens);
            } else {
                body["max_tokens"] = serde_json::json!(max_tokens);
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

    /// Send a streaming chat completion request.
    ///
    /// Sets `stream: true` in the request body and returns a channel receiver
    /// that yields `StreamChunk`s as they arrive from the LLM API.
    ///
    /// The returned `tokio::sync::mpsc::Receiver` will be closed when the
    /// stream ends (either naturally on `[DONE]` or on error).
    pub fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        let model = if model.is_empty() {
            self.config.default_model.clone()
        } else {
            model.to_string()
        };

        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let mut body = self.build_request_body(messages, tools, &model, options);
        body["stream"] = serde_json::json!(true);

        let api_key = self.config.api_key.clone();
        let headers = self.config.headers.clone();
        let client = self.client.clone();
        let provider_name = self.config.name.clone();

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let mut req = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json");

            for (key, value) in &headers {
                req = req.header(key.as_str(), value.as_str());
            }

            let resp = match req.json(&body).send().await {
                Ok(r) => r,
                Err(_) => {
                    let _ = tx
                        .send(Err(FailoverError::Timeout {
                            provider: provider_name,
                            model: model.clone(),
                        }))
                        .await;
                    return;
                }
            };

            let status = resp.status().as_u16();
            if status >= 400 {
                let text = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(FailoverError::from_status(
                        &provider_name,
                        &model,
                        status,
                        &text,
                    )))
                    .await;
                return;
            }

            // Parse the SSE stream from the response body.
            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            // Accumulated tool calls across chunks (index -> partial ToolCall).
            let mut pending_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();

            while let Some(chunk_result) = stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx
                            .send(Err(FailoverError::Format {
                                provider: provider_name.clone(),
                                message: e.to_string(),
                            }))
                            .await;
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // Process complete SSE lines.
                while let Some(pos) = buffer.find("\n\n") {
                    let block = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    for line in block.lines() {
                        let line = line.trim();
                        if !line.starts_with("data: ") {
                            continue;
                        }
                        let data = &line[6..];
                        if data.trim() == "[DONE]" {
                            let _ = tx.send(Ok(StreamChunk {
                                delta: String::new(),
                                tool_calls: vec![],
                                finish_reason: Some("stop".to_string()),
                                usage: None,
                            })).await;
                            return;
                        }

                        let parsed: serde_json::Value = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let delta_content = parsed["choices"][0]["delta"]["content"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();

                        let finish_reason = parsed["choices"][0]["finish_reason"]
                            .as_str()
                            .map(|s| s.to_string());

                        // Handle streaming tool calls.
                        if let Some(tc_array) =
                            parsed["choices"][0]["delta"]["tool_calls"].as_array()
                        {
                            for tc in tc_array {
                                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                                let entry = pending_tool_calls.entry(idx).or_insert_with(|| {
                                    (String::new(), String::new(), String::new())
                                });

                                if let Some(id) = tc["id"].as_str() {
                                    entry.0 = id.to_string();
                                }
                                if let Some(name) =
                                    tc["function"]["name"].as_str()
                                {
                                    entry.1 = name.to_string();
                                }
                                if let Some(args) =
                                    tc["function"]["arguments"].as_str()
                                {
                                    entry.2.push_str(args);
                                }
                            }
                        }

                        // Only emit a chunk if there's content, a finish, or accumulated tool calls at finish.
                        let tool_calls: Vec<ToolCall> = if finish_reason.is_some() {
                            pending_tool_calls
                                .iter()
                                .map(|(_, (id, name, args))| ToolCall {
                                    id: id.clone(),
                                    call_type: Some("function".to_string()),
                                    function: Some(FunctionCall {
                                        name: name.clone(),
                                        arguments: args.clone(),
                                    }),
                                    name: None,
                                    arguments: None,
                                })
                                .collect()
                        } else {
                            vec![]
                        };

                        if delta_content.is_empty()
                            && finish_reason.is_none()
                            && tool_calls.is_empty()
                        {
                            continue;
                        }

                        let usage = if finish_reason.is_some() {
                            parsed.get("usage").and_then(|u| {
                                Some(UsageInfo {
                                    prompt_tokens: u["prompt_tokens"].as_i64().unwrap_or(0),
                                    completion_tokens: u["completion_tokens"].as_i64().unwrap_or(0),
                                    total_tokens: u["total_tokens"].as_i64().unwrap_or(0),
                                })
                            })
                        } else {
                            None
                        };

                        let chunk = StreamChunk {
                            delta: delta_content,
                            tool_calls,
                            finish_reason,
                            usage,
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            // Receiver dropped, stop streaming.
                            return;
                        }
                    }
                }
            }
        });

        rx
    }
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// A chunk of streamed LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Incremental text content (may be empty for tool_call or usage chunks).
    #[serde(default)]
    pub delta: String,
    /// Tool calls being streamed (partial, accumulated across chunks).
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Finish reason — present only on the final chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Token usage — present only on the final chunk (if provided by the API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
}

#[async_trait]
impl LLMProvider for HttpProvider {
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

        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let body = self.build_request_body(messages, tools, model, options);

        let mut req = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json");

        for (key, value) in &self.config.headers {
            req = req.header(key.as_str(), value.as_str());
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|_e| FailoverError::Timeout {
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

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| FailoverError::Format {
                provider: self.config.name.clone(),
                message: e.to_string(),
            })?;

        // Parse OpenAI-compatible response
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let finish_reason = data["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("stop")
            .to_string();

        let usage = if let Some(usage_val) = data.get("usage") {
            Some(UsageInfo {
                prompt_tokens: usage_val["prompt_tokens"].as_i64().unwrap_or(0),
                completion_tokens: usage_val["completion_tokens"].as_i64().unwrap_or(0),
                total_tokens: usage_val["total_tokens"].as_i64().unwrap_or(0),
            })
        } else {
            None
        };

        let tool_calls = if let Some(tc_array) = data["choices"][0]["message"]["tool_calls"].as_array() {
            tc_array
                .iter()
                .filter_map(|tc| {
                    let id = tc["id"].as_str()?.to_string();
                    let name = tc["function"]["name"].as_str()?.to_string();
                    let arguments = tc["function"]["arguments"].as_str()?.to_string();
                    Some(ToolCall {
                        id,
                        call_type: Some("function".to_string()),
                        function: Some(FunctionCall { name, arguments }),
                        name: None,
                        arguments: None,
                    })
                })
                .collect()
        } else {
            vec![]
        };

        Ok(LLMResponse {
            content,
            tool_calls,
            finish_reason,
            usage,
        })
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        &self.config.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

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
    fn test_build_request_with_tools() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];

        let body = provider.build_request_body(&[], &tools, "gpt-4", &ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(1000),
            ..Default::default()
        });
        assert!(body.get("tools").is_some());
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 1000);
    }

    #[test]
    fn test_normalize_model() {
        assert_eq!(HttpProvider::normalize_model("openai/gpt-4"), "gpt-4");
        assert_eq!(HttpProvider::normalize_model("gpt4"), "gpt-4");
        assert_eq!(HttpProvider::normalize_model("gpt-4o"), "gpt-4o");
        assert_eq!(HttpProvider::normalize_model("gpt4o"), "gpt-4o");
        assert_eq!(HttpProvider::normalize_model("claude3"), "claude-3-sonnet-20240229");
        assert_eq!(HttpProvider::normalize_model("anthropic/claude3-opus"), "claude-3-opus-20240229");
        assert_eq!(HttpProvider::normalize_model("my-custom-model"), "my-custom-model");
        assert_eq!(HttpProvider::normalize_model("  gpt-4  "), "gpt-4");
    }

    #[test]
    fn test_uses_completion_tokens() {
        assert!(HttpProvider::uses_completion_tokens("o1-preview"));
        assert!(HttpProvider::uses_completion_tokens("o3-mini"));
        assert!(HttpProvider::uses_completion_tokens("gpt-5-turbo"));
        assert!(HttpProvider::uses_completion_tokens("glm-4"));
        assert!(!HttpProvider::uses_completion_tokens("gpt-4"));
        assert!(!HttpProvider::uses_completion_tokens("claude-3"));
    }

    #[test]
    fn test_build_request_body_normalizes_model() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(&[], &[], "openai/gpt-4", &ChatOptions::default());
        assert_eq!(body["model"], "gpt-4");
    }

    #[test]
    fn test_build_request_body_o1_no_temperature() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "o1".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(
            &[], &[], "o1-preview",
            &ChatOptions { temperature: Some(0.7), ..Default::default() }
        );
        // o1 models should NOT have temperature
        assert!(body.get("temperature").is_none());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_http_provider_config_serialization() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 60,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: HttpProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.api_key, "sk-test");
        assert_eq!(deserialized.timeout_secs, 60);
    }

    #[test]
    fn test_http_provider_config_with_proxy() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: Some("http://proxy:8080".to_string()),
            preserve_prefix: false,
        };
        // Should not panic when creating with proxy
        let _provider = HttpProvider::new(config);
    }

    #[test]
    fn test_http_provider_config_with_headers() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: {
                let mut h = HashMap::new();
                h.insert("X-Custom".to_string(), "value".to_string());
                h
            },
            proxy: None,
            preserve_prefix: false,
        };
        let _provider = HttpProvider::new(config);
    }

    #[test]
    fn test_normalize_model_aliases() {
        assert_eq!(HttpProvider::normalize_model("gpt4"), "gpt-4");
        assert_eq!(HttpProvider::normalize_model("gpt4o"), "gpt-4o");
        assert_eq!(HttpProvider::normalize_model("gpt4-turbo"), "gpt-4-turbo");
        assert_eq!(HttpProvider::normalize_model("gpt35-turbo"), "gpt-3.5-turbo");
        assert_eq!(HttpProvider::normalize_model("claude3"), "claude-3-sonnet-20240229");
        assert_eq!(HttpProvider::normalize_model("claude3-opus"), "claude-3-opus-20240229");
        assert_eq!(HttpProvider::normalize_model("claude3-sonnet"), "claude-3-sonnet-20240229");
        assert_eq!(HttpProvider::normalize_model("claude3-haiku"), "claude-3-haiku-20240307");
    }

    #[test]
    fn test_normalize_model_preserves_unknown() {
        assert_eq!(HttpProvider::normalize_model("my-custom-model"), "my-custom-model");
        assert_eq!(HttpProvider::normalize_model("deepseek-chat"), "deepseek-chat");
    }

    #[test]
    fn test_normalize_model_strips_prefix() {
        assert_eq!(HttpProvider::normalize_model("openai/gpt-4"), "gpt-4");
        assert_eq!(HttpProvider::normalize_model("anthropic/claude-3"), "claude-3");
        assert_eq!(HttpProvider::normalize_model("deepseek/deepseek-chat"), "deepseek-chat");
    }

    #[test]
    fn test_normalize_model_whitespace() {
        assert_eq!(HttpProvider::normalize_model("  gpt-4  "), "gpt-4");
        assert_eq!(HttpProvider::normalize_model("  openai/gpt-4  "), "gpt-4");
    }

    #[test]
    fn test_uses_completion_tokens_various() {
        assert!(HttpProvider::uses_completion_tokens("o1-preview"));
        assert!(HttpProvider::uses_completion_tokens("o1-mini"));
        assert!(HttpProvider::uses_completion_tokens("o3-mini"));
        assert!(HttpProvider::uses_completion_tokens("o3-high"));
        assert!(HttpProvider::uses_completion_tokens("gpt-5"));
        assert!(HttpProvider::uses_completion_tokens("gpt-5-turbo"));
        assert!(HttpProvider::uses_completion_tokens("glm-4"));
        assert!(HttpProvider::uses_completion_tokens("glm-4-plus"));
        assert!(!HttpProvider::uses_completion_tokens("gpt-4"));
        assert!(!HttpProvider::uses_completion_tokens("gpt-4o"));
        assert!(!HttpProvider::uses_completion_tokens("claude-3"));
        assert!(!HttpProvider::uses_completion_tokens("deepseek-chat"));
    }

    #[test]
    fn test_build_request_body_completion_tokens() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        // o1 model should use max_completion_tokens
        let body = provider.build_request_body(
            &[], &[], "o1-preview",
            &ChatOptions { max_tokens: Some(4096), ..Default::default() }
        );
        assert!(body.get("max_completion_tokens").is_some());
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["max_completion_tokens"], 4096);
    }

    #[test]
    fn test_build_request_body_regular_max_tokens() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        // Regular model uses max_tokens
        let body = provider.build_request_body(
            &[], &[], "gpt-4",
            &ChatOptions { max_tokens: Some(2048), ..Default::default() }
        );
        assert!(body.get("max_tokens").is_some());
        assert!(body.get("max_completion_tokens").is_none());
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn test_build_request_body_top_p() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(
            &[], &[], "gpt-4",
            &ChatOptions { top_p: Some(0.9), ..Default::default() }
        );
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn test_build_request_body_stop() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(
            &[], &[], "gpt-4",
            &ChatOptions { stop: Some(vec!["stop1".to_string(), "stop2".to_string()]), ..Default::default() }
        );
        assert!(body.get("stop").is_some());
    }

    #[test]
    fn test_build_request_body_no_optional_fields() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(&[], &[], "gpt-4", &ChatOptions::default());
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("stop").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn test_build_request_body_kimi_temperature() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        // Kimi models should auto-set temperature=1.0 when not specified
        let body = provider.build_request_body(
            &[], &[], "moonshot-v1",
            &ChatOptions::default()
        );
        // "moonshot" triggers Kimi logic
        assert_eq!(body["temperature"], 1.0);
    }

    #[test]
    fn test_build_request_body_preserve_prefix() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: true,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(&[], &[], "openai/gpt-4", &ChatOptions::default());
        assert_eq!(body["model"], "openai/gpt-4");
    }

    #[test]
    fn test_build_request_body_o3_no_temperature() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let body = provider.build_request_body(
            &[], &[], "o3-mini",
            &ChatOptions { temperature: Some(0.5), ..Default::default() }
        );
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn test_default_model_and_name() {
        let config = HttpProviderConfig {
            name: "my-provider".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4o".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);
        assert_eq!(provider.default_model(), "gpt-4o");
        assert_eq!(provider.name(), "my-provider");
    }

    #[test]
    fn test_build_request_body_with_multiple_messages() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-4".to_string(),
            timeout_secs: 30,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let messages = vec![
            Message { role: "system".to_string(), content: "You are helpful".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None },
            Message { role: "user".to_string(), content: "Hello".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None },
            Message { role: "assistant".to_string(), content: "Hi".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None },
            Message { role: "user".to_string(), content: "How are you?".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None },
        ];

        let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
        assert_eq!(body["messages"].as_array().unwrap().len(), 4);
    }

    // --- StreamChunk tests ---

    #[test]
    fn test_stream_chunk_serialize() {
        let chunk = StreamChunk {
            delta: "Hello".to_string(),
            tool_calls: vec![],
            finish_reason: None,
            usage: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("Hello"));
        assert!(!json.contains("finish_reason"));
    }

    #[test]
    fn test_stream_chunk_with_finish() {
        let chunk = StreamChunk {
            delta: String::new(),
            tool_calls: vec![],
            finish_reason: Some("stop".to_string()),
            usage: Some(UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            }),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("stop"));
        assert!(json.contains("30"));
    }

    #[test]
    fn test_stream_chunk_with_tool_calls() {
        let chunk = StreamChunk {
            delta: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_123".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path": "/test"}"#.to_string(),
                }),
                name: None,
                arguments: None,
            }],
            finish_reason: Some("tool_calls".to_string()),
            usage: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("call_123"));
        assert!(json.contains("read_file"));
        assert!(json.contains("tool_calls"));
    }

    #[test]
    fn test_stream_chunk_deserialize() {
        let json = r#"{"delta":" world","tool_calls":[],"finish_reason":null}"#;
        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.delta, " world");
        assert!(chunk.tool_calls.is_empty());
        assert!(chunk.finish_reason.is_none());
    }

    #[tokio::test]
    async fn test_chat_stream_returns_channel() {
        let config = HttpProviderConfig {
            name: "test".to_string(),
            base_url: "http://127.0.0.1:1".to_string(), // non-existent
            api_key: "test".to_string(),
            default_model: "test".to_string(),
            timeout_secs: 1,
            headers: HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        let provider = HttpProvider::new(config);

        let messages = vec![Message {
            role: "user".to_string(),
            content: "test".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];

        let mut rx = provider.chat_stream(&messages, &[], "test", &ChatOptions::default());

        // Should eventually get an error (connection refused / timeout).
        // Use tokio::time::timeout to avoid hanging.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            rx.recv(),
        ).await;

        // The channel should return something (likely an error).
        match result {
            Ok(Some(Err(_))) => { /* expected — connection error */ }
            Ok(Some(Ok(chunk))) => {
                // Got a chunk — unlikely with port 1, but not a failure
                assert!(!chunk.delta.is_empty() || chunk.finish_reason.is_some());
            }
            Ok(None) => { /* channel closed */ }
            Err(_) => { /* timeout — acceptable, the spawned task might be slow */ }
        }
    }
}

//! HTTP-based LLM provider (OpenAI-compatible).

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Extract UsageInfo from a usage JSON value, including cached token metrics.
fn extract_usage(u: &serde_json::Value) -> UsageInfo {
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
}

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
        tracing::info!(
            name = %config.name,
            base_url = %config.base_url,
            default_model = %config.default_model,
            timeout_secs = config.timeout_secs,
            "[Provider] Initialized HTTP provider"
        );
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

        tracing::debug!(
            provider = %provider_name,
            model = %model,
            url = %url,
            message_count = messages.len(),
            "[Provider] Starting streaming request"
        );

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
                tracing::error!(
                    provider = %provider_name,
                    model = %model,
                    status = status,
                    "[Provider] Streaming request failed with HTTP error"
                );
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
            // Accumulated reasoning content from thinking-mode models.
            let mut accumulated_reasoning = String::new();

            while let Some(chunk_result) = stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(
                            provider = %provider_name,
                            error = %e,
                            "[Provider] SSE stream read error"
                        );
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
                            tracing::debug!(
                                provider = %provider_name,
                                model = %model,
                                "[Provider] SSE stream completed ([DONE])"
                            );
                            let _ = tx.send(Ok(StreamChunk {
                                delta: String::new(),
                                tool_calls: vec![],
                                finish_reason: Some("stop".to_string()),
                                usage: None,
                                reasoning_content: if accumulated_reasoning.is_empty() { None } else { Some(accumulated_reasoning.clone()) },
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

                        // Accumulate reasoning_content from thinking-mode models.
                        if let Some(rc) = parsed["choices"][0]["delta"]["reasoning_content"].as_str() {
                            accumulated_reasoning.push_str(rc);
                        }

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
                            parsed.get("usage").map(|u| extract_usage(u))
                        } else {
                            None
                        };

                        let chunk = StreamChunk {
                            delta: delta_content,
                            tool_calls,
                            finish_reason: finish_reason.clone(),
                            usage,
                            reasoning_content: if finish_reason.is_some() && !accumulated_reasoning.is_empty() {
                                Some(accumulated_reasoning.clone())
                            } else {
                                None
                            },
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
    /// Accumulated reasoning content from thinking-mode models.
    /// Present only on the final chunk (not streamed incrementally).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl Default for StreamChunk {
    fn default() -> Self {
        Self {
            delta: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            usage: None,
            reasoning_content: None,
        }
    }
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

        tracing::debug!(
            provider = %self.config.name,
            model = %model,
            message_count = messages.len(),
            tool_count = tools.len(),
            "[Provider] Sending non-streaming request"
        );

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
            tracing::error!(
                provider = %self.config.name,
                model = model,
                status = status,
                response = %text.chars().take(200).collect::<String>(),
                "[Provider] Request failed with HTTP error"
            );
            return Err(FailoverError::from_status(
                &self.config.name,
                model,
                status,
                &text,
            ));
        }

        let raw_response_text = resp
            .text()
            .await
            .map_err(|e| FailoverError::Format {
                provider: self.config.name.clone(),
                message: e.to_string(),
            })?;
        let data: serde_json::Value = serde_json::from_str(&raw_response_text)
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

        let usage = data.get("usage").map(|u| extract_usage(u));

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

        tracing::debug!(
            provider = %self.config.name,
            model = model,
            finish_reason = %finish_reason,
            tool_call_count = tool_calls.len(),
            prompt_tokens = usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
            completion_tokens = usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
            total_tokens = usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
            "[Provider] Response received"
        );

        Ok(LLMResponse {
            content,
            tool_calls,
            finish_reason,
            usage,
            reasoning_content: data["choices"][0]["message"]["reasoning_content"]
                .as_str()
                .map(|s| s.to_string()),
            extra: HashMap::new(),
            raw_request_body: Some(body),
            raw_response_body: Some(raw_response_text),
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
mod tests;

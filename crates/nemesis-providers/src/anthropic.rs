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
        Some(UsageInfo {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
        })
    } else {
        None
    };

    LLMResponse {
        content,
        tool_calls,
        finish_reason: finish_reason.to_string(),
        usage,
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
mod tests {
    use super::*;

    #[test]
    fn test_normalize_base_url() {
        assert_eq!(normalize_base_url(""), DEFAULT_BASE_URL);
        assert_eq!(normalize_base_url("https://api.anthropic.com/v1"), "https://api.anthropic.com");
        assert_eq!(normalize_base_url("https://custom.api.com/"), "https://custom.api.com");
        assert_eq!(normalize_base_url("  https://api.anthropic.com/v1/  "), "https://api.anthropic.com");
    }

    #[test]
    fn test_build_request_body_simple() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        assert_eq!(body["model"], "claude-3");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 4096);
    }

    #[test]
    fn test_build_request_body_with_system() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "You are helpful".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "user".to_string(),
                content: "Hi".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        assert!(body.get("system").is_some());
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 1);
        assert_eq!(system[0]["type"], "text");
    }

    #[test]
    fn test_translate_tools() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }),
            },
        }];
        let translated = translate_tools(&tools);
        assert_eq!(translated.len(), 1);
        assert_eq!(translated[0]["name"], "read_file");
        assert_eq!(translated[0]["description"], "Read a file");
        assert!(translated[0].get("input_schema").is_some());
    }

    #[test]
    fn test_parse_response_text_only() {
        let data = serde_json::json!({
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.finish_reason, "stop");
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn test_parse_response_tool_use() {
        let data = serde_json::json!({
            "content": [
                {"type": "text", "text": "Using tool"},
                {"type": "tool_use", "id": "tu_123", "name": "read_file", "input": {"path": "/tmp"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 10}
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "Using tool");
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "tu_123");
        assert_eq!(resp.tool_calls[0].name.as_deref(), Some("read_file"));
    }

    #[test]
    fn test_parse_response_max_tokens() {
        let data = serde_json::json!({
            "content": [{"type": "text", "text": "Truncated"}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 10, "output_tokens": 100}
        });
        let resp = parse_response(&data);
        assert_eq!(resp.finish_reason, "length");
    }

    #[test]
    fn test_anthropic_config_default() {
        let config = AnthropicConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.default_model, DEFAULT_MODEL);
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_with_token_source_and_base_url() {
        let config = AnthropicConfig::default();
        let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
            Box::new(|| Ok("refreshed-token".to_string()));
        let provider = AnthropicProvider::with_token_source_and_base_url(
            config,
            ts,
            "https://custom.api.com/v1/",
        );
        assert_eq!(provider.base_url(), "https://custom.api.com");
        assert!(provider.token_source.is_some());
    }

    #[test]
    fn test_with_token_source_and_base_url_empty() {
        let config = AnthropicConfig::default();
        let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
            Box::new(|| Ok("token".to_string()));
        let provider = AnthropicProvider::with_token_source_and_base_url(config, ts, "");
        // Empty base_url should keep the config default
        assert_eq!(provider.base_url(), DEFAULT_BASE_URL);
    }

    #[test]
    fn test_base_url_method() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        assert_eq!(provider.base_url(), DEFAULT_BASE_URL);
    }

    // -- Additional tests --

    #[test]
    fn test_anthropic_config_serialization_roundtrip() {
        let config = AnthropicConfig {
            api_key: "sk-ant-test".into(),
            base_url: "https://custom.api.com".into(),
            default_model: "claude-3-opus".into(),
            timeout_secs: 60,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: AnthropicConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.api_key, "sk-ant-test");
        assert_eq!(back.base_url, "https://custom.api.com");
        assert_eq!(back.default_model, "claude-3-opus");
        assert_eq!(back.timeout_secs, 60);
    }

    #[test]
    fn test_anthropic_config_deserialization_partial() {
        let json = r#"{"api_key": "sk-ant-test"}"#;
        let config: AnthropicConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.api_key, "sk-ant-test");
        assert_eq!(config.base_url, ""); // serde default = empty string
        assert_eq!(config.default_model, ""); // serde default = empty string
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_normalize_base_url_trailing_slash() {
        assert_eq!(normalize_base_url("https://api.anthropic.com/"), "https://api.anthropic.com");
        // /v1 gets stripped too
        assert_eq!(normalize_base_url("https://api.anthropic.com/v1/"), "https://api.anthropic.com");
        assert_eq!(normalize_base_url("https://api.anthropic.com/v1"), "https://api.anthropic.com");
    }

    #[test]
    fn test_normalize_base_url_no_trailing_slash() {
        assert_eq!(normalize_base_url("https://api.anthropic.com"), "https://api.anthropic.com");
    }

    #[test]
    fn test_normalize_base_url_empty() {
        assert_eq!(normalize_base_url(""), DEFAULT_BASE_URL);
    }

    #[test]
    fn test_parse_response_no_usage() {
        let data = serde_json::json!({
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn"
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "Hello!");
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_parse_response_empty_content() {
        let data = serde_json::json!({
            "content": [],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "");
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_response_text_and_tool_use() {
        let data = serde_json::json!({
            "content": [
                {"type": "text", "text": "Let me check"},
                {"type": "tool_use", "id": "tu_1", "name": "search", "input": {"q": "test"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 15, "output_tokens": 8}
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "Let me check");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.usage.unwrap().total_tokens, 23);
    }

    #[test]
    fn test_translate_tools_empty() {
        let tools: Vec<ToolDefinition> = vec![];
        let translated = translate_tools(&tools);
        assert!(translated.is_empty());
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_BASE_URL, "https://api.anthropic.com");
        assert_eq!(DEFAULT_MODEL, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn test_provider_name() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn test_provider_default_model() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    // ---- Additional coverage for edge cases ----

    #[test]
    fn test_build_request_body_user_with_tool_call_id() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "file result data".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("tu_123".to_string()),
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tu_123");
    }

    #[test]
    fn test_build_request_body_tool_with_call_id() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let messages = vec![Message {
            role: "tool".to_string(),
            content: "tool output".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("tu_456".to_string()),
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tu_456");
    }

    #[test]
    fn test_build_request_body_tool_without_call_id() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let messages = vec![Message {
            role: "tool".to_string(),
            content: "orphan output".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        let msgs = body["messages"].as_array().unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_build_request_body_unknown_role() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let messages = vec![Message {
            role: "custom_role".to_string(),
            content: "ignored".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        let msgs = body["messages"].as_array().unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_build_request_body_with_temperature() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let body = provider.build_request_body(&[], &[], "claude-3", &ChatOptions {
            temperature: Some(0.5),
            ..Default::default()
        });
        assert_eq!(body["temperature"], 0.5);
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "test_tool".to_string(),
                description: "A test tool".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]}),
            },
        }];
        let body = provider.build_request_body(&[], &tools, "claude-3", &ChatOptions::default());
        assert!(body.get("tools").is_some());
        let tools_arr = body["tools"].as_array().unwrap();
        assert_eq!(tools_arr.len(), 1);
        assert_eq!(tools_arr[0]["name"], "test_tool");
        assert!(tools_arr[0]["input_schema"].get("required").is_some());
    }

    #[test]
    fn test_translate_tools_non_function_skipped() {
        let tools = vec![ToolDefinition {
            tool_type: "other".to_string(),
            function: ToolFunctionDefinition {
                name: "skipped".to_string(),
                description: "Should be skipped".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools(&tools);
        assert!(translated.is_empty());
    }

    #[test]
    fn test_translate_tools_no_description() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "no_desc".to_string(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools(&tools);
        assert_eq!(translated.len(), 1);
        assert!(translated[0].get("description").is_none());
    }

    #[test]
    fn test_translate_tools_no_required_field() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "no_req".to_string(),
                description: "No required field".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
            },
        }];
        let translated = translate_tools(&tools);
        assert_eq!(translated.len(), 1);
        assert!(translated[0]["input_schema"].get("required").is_none());
    }

    #[test]
    fn test_parse_response_tool_use_with_invalid_input() {
        let data = serde_json::json!({
            "content": [
                {"type": "tool_use", "id": "tu_bad", "name": "test", "input": "not an object"}
            ],
            "stop_reason": "tool_use"
        });
        let resp = parse_response(&data);
        assert_eq!(resp.tool_calls.len(), 1);
        // Invalid input should get raw fallback
        assert!(resp.tool_calls[0].arguments.is_some());
        assert!(resp.tool_calls[0].arguments.as_ref().unwrap().contains_key("raw"));
    }

    #[test]
    fn test_parse_response_unknown_block_type() {
        let data = serde_json::json!({
            "content": [
                {"type": "unknown_block", "data": "something"}
            ],
            "stop_reason": "end_turn"
        });
        let resp = parse_response(&data);
        assert_eq!(resp.content, "");
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_response_stop_reason_end_turn() {
        let data = serde_json::json!({
            "content": [{"type": "text", "text": "done"}],
            "stop_reason": "end_turn"
        });
        let resp = parse_response(&data);
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn test_parse_response_stop_reason_unknown() {
        let data = serde_json::json!({
            "content": [{"type": "text", "text": "done"}],
            "stop_reason": "unknown_reason"
        });
        let resp = parse_response(&data);
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn test_parse_response_no_stop_reason() {
        let data = serde_json::json!({
            "content": [{"type": "text", "text": "no stop"}]
        });
        let resp = parse_response(&data);
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn test_get_api_key_no_token_source() {
        let provider = AnthropicProvider::new(AnthropicConfig {
            api_key: "direct-key".to_string(),
            ..Default::default()
        });
        assert_eq!(provider.get_api_key().unwrap(), "direct-key");
    }

    #[test]
    fn test_get_api_key_with_token_source() {
        let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
            Box::new(|| Ok("dynamic-key".to_string()));
        let provider = AnthropicProvider::with_token_source(
            AnthropicConfig::default(),
            ts,
        );
        assert_eq!(provider.get_api_key().unwrap(), "dynamic-key");
    }

    #[test]
    fn test_get_api_key_with_failing_token_source() {
        let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
            Box::new(|| Err("token refresh failed".to_string()));
        let provider = AnthropicProvider::with_token_source(
            AnthropicConfig::default(),
            ts,
        );
        assert!(provider.get_api_key().is_err());
    }

    #[test]
    fn test_normalize_base_url_only_v1() {
        assert_eq!(normalize_base_url("/v1"), DEFAULT_BASE_URL);
        assert_eq!(normalize_base_url("  /v1/  "), DEFAULT_BASE_URL);
    }

    #[test]
    fn test_assistant_with_tool_calls_and_function_fallback() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        // ToolCall with no name field, but has function.name
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "tc_1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "search".to_string(),
                    arguments: r#"{"q":"test"}"#.to_string(),
                }),
                name: None, // name is None, should fallback to function.name
                arguments: None, // arguments is None, should produce empty json
            }],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        let content = msgs[0]["content"].as_array().unwrap();
        // Empty content should not produce text block, only tool_use block
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "search"); // from function.name fallback
    }
}

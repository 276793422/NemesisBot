//! Codex/OpenAI provider (Responses API streaming).

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const CODEX_DEFAULT_MODEL: &str = "gpt-5.2";
const CODEX_DEFAULT_INSTRUCTIONS: &str = "You are Codex, a coding assistant.";
const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Codex provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexConfig {
    pub api_key: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enable_web_search: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    120
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            account_id: String::new(),
            default_model: CODEX_DEFAULT_MODEL.to_string(),
            base_url: CODEX_BASE_URL.to_string(),
            enable_web_search: true,
            timeout_secs: 120,
        }
    }
}

/// Codex/OpenAI Responses API provider.
pub struct CodexProvider {
    config: CodexConfig,
    client: reqwest::Client,
    token_source:
        Option<Box<dyn Fn() -> Result<(String, String), String> + Send + Sync>>,
}

impl CodexProvider {
    pub fn new(config: CodexConfig) -> Self {
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

    /// Create with a token source for OAuth token refresh.
    /// The source returns `(token, account_id)`.
    pub fn with_token_source(
        config: CodexConfig,
        token_source: Box<dyn Fn() -> Result<(String, String), String> + Send + Sync>,
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

    /// Build the Responses API request body.
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> serde_json::Value {
        let mut input_items = Vec::new();
        let mut instructions = String::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    instructions = msg.content.clone();
                }
                "user" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        input_items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": tc_id,
                            "output": msg.content
                        }));
                    } else {
                        input_items.push(serde_json::json!({
                            "type": "message",
                            "role": "user",
                            "content": msg.content
                        }));
                    }
                }
                "assistant" => {
                    if !msg.tool_calls.is_empty() {
                        if !msg.content.is_empty() {
                            input_items.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": msg.content
                            }));
                        }
                        for tc in &msg.tool_calls {
                            let name = tc.name.as_deref()
                                .or_else(|| tc.function.as_ref().map(|f| f.name.as_str()))
                                .unwrap_or("");
                            let args = tc.function.as_ref()
                                .map(|f| f.arguments.as_str())
                                .unwrap_or("{}");
                            input_items.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": name,
                                "arguments": args
                            }));
                        }
                    } else {
                        input_items.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": msg.content
                        }));
                    }
                }
                "tool" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        input_items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": tc_id,
                            "output": msg.content
                        }));
                    }
                }
                _ => {}
            }
        }

        let effective_instructions = if instructions.is_empty() {
            CODEX_DEFAULT_INSTRUCTIONS
        } else {
            &instructions
        };

        let mut body = serde_json::json!({
            "model": model,
            "input": input_items,
            "instructions": effective_instructions,
            "store": false,
        });

        if let Some(max_tokens) = options.max_tokens {
            body["max_output_tokens"] = serde_json::json!(max_tokens);
        }

        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        // Translate tools
        if !tools.is_empty() || self.config.enable_web_search {
            let api_tools = translate_tools_for_codex(tools, self.config.enable_web_search);
            if !api_tools.is_empty() {
                body["tools"] = serde_json::json!(api_tools);
            }
        }

        body
    }
}

/// Result of resolving a model name for the Codex backend.
#[derive(Debug, Clone)]
pub struct ResolvedCodexModel {
    /// The resolved model name.
    pub model: String,
    /// If non-empty, explains why the original model was replaced with a fallback.
    pub fallback_reason: String,
}

/// Resolve a model name for the Codex backend, returning both the resolved
/// model and an optional fallback reason.
///
/// This is the public equivalent of the internal resolve logic with
/// additional diagnostic information about why a model was mapped to a fallback.
///
/// # Logic
/// 1. Empty model -> default (`gpt-5.2`), reason "empty model"
/// 2. `openai/` prefix -> strip it
/// 3. Any other namespace (`vendor/model`) -> default, reason "non-openai model namespace"
/// 4. Unsupported prefixes (claude, gemini, deepseek, etc.) -> default, reason "unsupported model prefix"
/// 5. Supported families (`gpt-*`, `o3*`, `o4*`) -> pass through, no reason
/// 6. Anything else -> default, reason "unsupported model family"
pub fn resolve_codex_model(model: &str) -> ResolvedCodexModel {
    let m = model.trim().to_lowercase();
    if m.is_empty() {
        return ResolvedCodexModel {
            model: CODEX_DEFAULT_MODEL.to_string(),
            fallback_reason: "empty model".to_string(),
        };
    }

    // Strip openai/ prefix
    let m = if m.starts_with("openai/") {
        &m[7..]
    } else if m.contains('/') {
        return ResolvedCodexModel {
            model: CODEX_DEFAULT_MODEL.to_string(),
            fallback_reason: "non-openai model namespace".to_string(),
        };
    } else {
        &m
    };

    // Unsupported model prefixes
    let unsupported = [
        "glm", "claude", "anthropic", "gemini", "google", "moonshot", "kimi",
        "qwen", "deepseek", "llama", "meta-llama", "mistral", "grok", "xai",
        "zhipu",
    ];
    for prefix in &unsupported {
        if m.starts_with(prefix) {
            return ResolvedCodexModel {
                model: CODEX_DEFAULT_MODEL.to_string(),
                fallback_reason: "unsupported model prefix".to_string(),
            };
        }
    }

    if m.starts_with("gpt-") || m.starts_with("o3") || m.starts_with("o4") {
        return ResolvedCodexModel {
            model: m.to_string(),
            fallback_reason: String::new(),
        };
    }

    ResolvedCodexModel {
        model: CODEX_DEFAULT_MODEL.to_string(),
        fallback_reason: "unsupported model family".to_string(),
    }
}

/// Translate tool definitions for the Codex/Responses API format.
fn translate_tools_for_codex(tools: &[ToolDefinition], enable_web_search: bool) -> Vec<serde_json::Value> {
    let mut result = Vec::new();

    for t in tools {
        if t.tool_type != "function" {
            continue;
        }
        // Skip web_search if we add it ourselves
        if enable_web_search && t.function.name.eq_ignore_ascii_case("web_search") {
            continue;
        }
        let mut ft = serde_json::json!({
            "type": "function",
            "name": t.function.name,
            "parameters": t.function.parameters,
            "strict": false,
        });
        if !t.function.description.is_empty() {
            ft["description"] = serde_json::json!(t.function.description);
        }
        result.push(ft);
    }

    if enable_web_search {
        result.push(serde_json::json!({
            "type": "web_search"
        }));
    }

    result
}

/// Parse the Codex Responses API response.
fn parse_codex_response(data: &serde_json::Value) -> LLMResponse {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(output) = data.get("output").and_then(|o| o.as_array()) {
        for item in output {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "message" => {
                    if let Some(content_arr) = item.get("content").and_then(|c| c.as_array()) {
                        for c in content_arr {
                            if c.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                                if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                                    content.push_str(text);
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    let id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let arguments_str = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");

                    let arguments: HashMap<String, serde_json::Value> =
                        serde_json::from_str(arguments_str).unwrap_or_else(|_| {
                            let mut m = HashMap::new();
                            m.insert("raw".to_string(), serde_json::Value::String(arguments_str.to_string()));
                            m
                        });

                    tool_calls.push(ToolCall {
                        id,
                        call_type: Some("function_call".to_string()),
                        function: Some(FunctionCall {
                            name: name.clone(),
                            arguments: arguments_str.to_string(),
                        }),
                        name: Some(name),
                        arguments: Some(arguments),
                    });
                }
                _ => {}
            }
        }
    }

    let finish_reason = if data.get("status").and_then(|s| s.as_str()) == Some("incomplete") {
        "length"
    } else if !tool_calls.is_empty() {
        "tool_calls"
    } else {
        "stop"
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

#[async_trait]
impl LLMProvider for CodexProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let resolved_model = if model.is_empty() {
            &self.config.default_model
        } else {
            model
        };
        let resolved_model = resolve_codex_model(resolved_model).model;

        let (api_key, account_id) = if let Some(ref ts) = self.token_source {
            ts().map_err(|_| FailoverError::Auth {
                provider: "codex".to_string(),
                model: resolved_model.clone(),
                status: 0,
            })?
        } else {
            (self.config.api_key.clone(), self.config.account_id.clone())
        };

        let url = format!(
            "{}/responses",
            self.config.base_url.trim_end_matches('/')
        );
        let body = self.build_request_body(messages, tools, &resolved_model, options);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("originator", "codex_cli_rs")
            .header("OpenAI-Beta", "responses=experimental");

        if !account_id.is_empty() {
            req = req.header("Chatgpt-Account-Id", &account_id);
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|_| FailoverError::Timeout {
                provider: "codex".to_string(),
                model: resolved_model.clone(),
            })?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            return Err(FailoverError::from_status("codex", &resolved_model, status, &text));
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| FailoverError::Format {
            provider: "codex".to_string(),
            message: e.to_string(),
        })?;

        Ok(parse_codex_response(&data))
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        "codex"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_model_default() {
        let r = resolve_codex_model("");
        assert_eq!(r.model, CODEX_DEFAULT_MODEL);
        assert_eq!(r.fallback_reason, "empty model");
        let r = resolve_codex_model("codex-cli");
        assert_eq!(r.model, CODEX_DEFAULT_MODEL);
    }

    #[test]
    fn test_resolve_model_openai_prefix() {
        let r = resolve_codex_model("openai/gpt-4o");
        assert_eq!(r.model, "gpt-4o");
        assert!(r.fallback_reason.is_empty());
    }

    #[test]
    fn test_resolve_model_unsupported() {
        let r = resolve_codex_model("anthropic/claude-3");
        assert_eq!(r.model, CODEX_DEFAULT_MODEL);
        assert_eq!(r.fallback_reason, "non-openai model namespace");
        let r = resolve_codex_model("deepseek/chat");
        assert_eq!(r.model, CODEX_DEFAULT_MODEL);
    }

    #[test]
    fn test_resolve_model_supported() {
        let r = resolve_codex_model("gpt-4o");
        assert_eq!(r.model, "gpt-4o");
        assert!(r.fallback_reason.is_empty());
        let r = resolve_codex_model("o3-mini");
        assert_eq!(r.model, "o3-mini");
        assert!(r.fallback_reason.is_empty());
        let r = resolve_codex_model("o4-mini");
        assert_eq!(r.model, "o4-mini");
        assert!(r.fallback_reason.is_empty());
    }

    #[test]
    fn test_translate_tools_for_codex() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools_for_codex(&tools, true);
        assert_eq!(translated.len(), 2); // function + web_search
        assert_eq!(translated[0]["type"], "function");
        assert_eq!(translated[1]["type"], "web_search");
    }

    #[test]
    fn test_translate_tools_skips_web_search() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "web_search".to_string(),
                description: "Search web".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools_for_codex(&tools, true);
        assert_eq!(translated.len(), 1); // only the built-in web_search
        assert_eq!(translated[0]["type"], "web_search");
    }

    #[test]
    fn test_parse_codex_response_text() {
        let data = serde_json::json!({
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "Hello!"}]
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.finish_reason, "stop");
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn test_parse_codex_response_function_call() {
        let data = serde_json::json!({
            "output": [
                {
                    "type": "function_call",
                    "call_id": "fc_123",
                    "name": "read_file",
                    "arguments": "{\"path\":\"/tmp\"}"
                }
            ],
            "usage": {"input_tokens": 20, "output_tokens": 10}
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "fc_123");
        assert_eq!(resp.finish_reason, "tool_calls");
    }

    #[test]
    fn test_parse_codex_response_incomplete() {
        let data = serde_json::json!({
            "status": "incomplete",
            "output": [],
            "usage": {"input_tokens": 10, "output_tokens": 0}
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.finish_reason, "length");
    }

    #[test]
    fn test_build_request_body() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["instructions"], CODEX_DEFAULT_INSTRUCTIONS);
        assert_eq!(body["store"], false);
    }

    #[test]
    fn test_config_default() {
        let config = CodexConfig::default();
        assert_eq!(config.default_model, CODEX_DEFAULT_MODEL);
        assert_eq!(config.base_url, CODEX_BASE_URL);
        assert!(config.enable_web_search);
    }

    // -- Additional tests --

    #[test]
    fn test_codex_config_serialization_roundtrip() {
        let config = CodexConfig {
            default_model: "gpt-4o".into(),
            base_url: "https://api.example.com".into(),
            api_key: "sk-test".into(),
            account_id: "acct-123".into(),
            enable_web_search: false,
            timeout_secs: 120,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: CodexConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.default_model, "gpt-4o");
        assert_eq!(back.base_url, "https://api.example.com");
        assert_eq!(back.api_key, "sk-test");
        assert_eq!(back.account_id, "acct-123");
        assert!(!back.enable_web_search);
    }

    #[test]
    fn test_codex_config_deserialization_partial() {
        // Fields with #[serde(default)] get empty string defaults, not CODEX_BASE_URL
        let json = r#"{"api_key": "sk-test", "default_model": "o3-mini"}"#;
        let config: CodexConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.default_model, "o3-mini");
        assert_eq!(config.base_url, ""); // serde default = empty string
        assert!(config.enable_web_search); // default_true
    }

    #[test]
    fn test_resolve_model_with_provider_prefix() {
        let r = resolve_codex_model("openai/o4-mini");
        assert_eq!(r.model, "o4-mini");
        assert!(r.fallback_reason.is_empty());
    }

    #[test]
    fn test_translate_tools_no_web_search() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools_for_codex(&tools, false);
        // Without web_search enabled, should only have the function tool
        assert_eq!(translated.len(), 1);
        assert_eq!(translated[0]["type"], "function");
    }

    #[test]
    fn test_parse_codex_response_empty_output() {
        let data = serde_json::json!({
            "output": [],
            "usage": {"input_tokens": 5, "output_tokens": 0}
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.content, "");
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn test_parse_codex_response_no_usage() {
        let data = serde_json::json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "Hi!"}]}
            ]
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.content, "Hi!");
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "test_tool".to_string(),
                description: "A test tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let body = provider.build_request_body(&messages, &tools, "gpt-4o", &ChatOptions::default());
        assert!(body.get("tools").is_some());
        let tools_arr = body["tools"].as_array().unwrap();
        assert!(!tools_arr.is_empty());
    }

    #[test]
    fn test_default_model_constant() {
        assert_eq!(CODEX_DEFAULT_MODEL, "gpt-5.2");
    }

    #[test]
    fn test_base_url_constant() {
        assert_eq!(CODEX_BASE_URL, "https://chatgpt.com/backend-api/codex");
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_build_request_body_with_system_message() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "You are a code reviewer".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "user".to_string(),
                content: "Review this code".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        assert_eq!(body["instructions"], "You are a code reviewer");
    }

    #[test]
    fn test_build_request_body_with_assistant_tool_calls() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "Let me read the file".to_string(),
                tool_calls: vec![ToolCall {
                    id: "call_1".to_string(),
                    call_type: Some("function".to_string()),
                    function: Some(FunctionCall {
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"/tmp/test"}"#.to_string(),
                    }),
                    name: Some("read_file".to_string()),
                    arguments: None,
                }],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        // Should have: message + function_call
        assert!(input.len() >= 1);
    }

    #[test]
    fn test_build_request_body_with_assistant_tool_calls_empty_content() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                }),
                name: None,
                arguments: None,
            }],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        // Empty content should not produce a message item, only function_call
        assert!(input.len() >= 1);
    }

    #[test]
    fn test_build_request_body_with_assistant_no_tool_calls() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: "I can help with that".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "assistant");
    }

    #[test]
    fn test_build_request_body_with_user_tool_call_id() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "file contents here".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("call_1".to_string()),
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_1");
    }

    #[test]
    fn test_build_request_body_with_tool_message() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "tool".to_string(),
            content: "tool result".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("call_1".to_string()),
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "function_call_output");
    }

    #[test]
    fn test_build_request_body_with_tool_message_no_call_id() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "tool".to_string(),
            content: "orphan result".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        assert!(input.is_empty());
    }

    #[test]
    fn test_build_request_body_with_options() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(
            &messages,
            &[],
            "gpt-4o",
            &ChatOptions {
                max_tokens: Some(2048),
                temperature: Some(0.7),
                ..Default::default()
            },
        );
        assert_eq!(body["max_output_tokens"], 2048);
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn test_build_request_body_with_unknown_role() {
        let provider = CodexProvider::new(CodexConfig::default());
        let messages = vec![Message {
            role: "custom_role".to_string(),
            content: "custom content".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
        let input = body["input"].as_array().unwrap();
        assert!(input.is_empty()); // Unknown roles are skipped
    }

    #[test]
    fn test_translate_tools_for_codex_non_function_type() {
        let tools = vec![ToolDefinition {
            tool_type: "non_function".to_string(),
            function: ToolFunctionDefinition {
                name: "ignored".to_string(),
                description: "Should be ignored".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools_for_codex(&tools, false);
        assert!(translated.is_empty()); // non-function tools are skipped
    }

    #[test]
    fn test_translate_tools_for_codex_no_description() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "no_desc".to_string(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let translated = translate_tools_for_codex(&tools, false);
        assert_eq!(translated.len(), 1);
        assert!(translated[0].get("description").is_none());
    }

    #[test]
    fn test_parse_codex_response_function_call_bad_args() {
        let data = serde_json::json!({
            "output": [
                {
                    "type": "function_call",
                    "call_id": "fc_bad",
                    "name": "bad_args",
                    "arguments": "not valid json"
                }
            ]
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.tool_calls.len(), 1);
        // Arguments that fail to parse should get raw fallback
        assert!(resp.tool_calls[0].arguments.is_some());
        assert!(resp.tool_calls[0].arguments.as_ref().unwrap().contains_key("raw"));
    }

    #[test]
    fn test_parse_codex_response_unknown_output_type() {
        let data = serde_json::json!({
            "output": [
                {
                    "type": "unknown_type",
                    "data": "something"
                }
            ]
        });
        let resp = parse_codex_response(&data);
        assert_eq!(resp.content, "");
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_resolve_codex_model_all_unsupported_prefixes() {
        let unsupported = [
            "glm-4", "claude-3", "anthropic-1", "gemini-pro", "google-1",
            "moonshot-v1", "kimi-chat", "qwen-7b", "deepseek-chat",
            "llama-3", "meta-llama-3", "mistral-7b", "grok-1", "xai-1", "zhipu-4",
        ];
        for model in &unsupported {
            let r = resolve_codex_model(model);
            assert_eq!(r.model, CODEX_DEFAULT_MODEL, "Expected default for {}", model);
            assert_eq!(r.fallback_reason, "unsupported model prefix", "Expected prefix reason for {}", model);
        }
    }

    #[test]
    fn test_resolve_codex_model_unsupported_family() {
        let r = resolve_codex_model("my-custom-model");
        assert_eq!(r.model, CODEX_DEFAULT_MODEL);
        assert_eq!(r.fallback_reason, "unsupported model family");
    }

    #[test]
    fn test_resolve_codex_model_whitespace() {
        let r = resolve_codex_model("  gpt-4o  ");
        assert_eq!(r.model, "gpt-4o");
        assert!(r.fallback_reason.is_empty());
    }

    #[test]
    fn test_resolve_codex_model_case_insensitive() {
        let r = resolve_codex_model("GPT-4O");
        assert_eq!(r.model, "gpt-4o");
        assert!(r.fallback_reason.is_empty());
    }

    #[test]
    fn test_codex_with_token_source() {
        let ts: Box<dyn Fn() -> Result<(String, String), String> + Send + Sync> =
            Box::new(|| Ok(("token".to_string(), "account".to_string())));
        let provider = CodexProvider::with_token_source(CodexConfig::default(), ts);
        assert_eq!(provider.name(), "codex");
        assert_eq!(provider.default_model(), CODEX_DEFAULT_MODEL);
    }

    #[test]
    fn test_resolved_codex_model_debug() {
        let r = resolve_codex_model("gpt-4o");
        let debug = format!("{:?}", r);
        assert!(debug.contains("gpt-4o"));
    }
}

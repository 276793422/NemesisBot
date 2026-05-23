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
            cached_tokens: None,
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
mod tests;

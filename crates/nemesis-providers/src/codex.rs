//! Codex/OpenAI provider (Responses API streaming).

use crate::failover::{FailoverError, transport_error};
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
    /// 出站代理 URL（代理接线修复 2026-09-17）：http/socks5；None = 直连。
    /// 来自模型条目 `proxy` 字段经 FactoryConfig 透传；非法 URL warn 回落
    /// 直连（与 HttpProvider/Anthropic 同策略）。
    #[serde(default)]
    pub proxy: Option<String>,
    /// per-model 流式开关（2026-10-13 stream 键）：空（auto）= 流式（默认，
    /// 墙钟根因修复的形态）；"no" = 逃生门强制非流式（body 不带 stream 字段，
    /// 等完整响应）。经 FactoryConfig.stream_mode 透传，构造期消费。
    #[serde(default)]
    pub stream_mode: String,
}

fn default_true() -> bool {
    true
}

/// reqwest client 构造单一出口（timeout + 可选出站代理；非法代理 warn
/// 回落直连——与 HttpProvider/Anthropic 同策略）。
fn build_client(timeout_secs: u64, proxy: Option<&str>) -> reqwest::Client {
    let mut builder =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(timeout_secs));
    if let Some(proxy_url) = proxy
        && !proxy_url.is_empty()
    {
        match reqwest::Proxy::all(proxy_url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => {
                tracing::warn!(
                    proxy = %proxy_url,
                    error = %e,
                    "Codex provider: invalid proxy URL, using direct connection"
                );
            }
        }
    }
    builder.build().expect("failed to build HTTP client")
}

// P3A 超时对齐（2026-09-12）：与 anthropic/http lane 统一 600s（原 120s，
// 超时阶梯口径分裂的一半）。per-model 覆盖经模型条目 extra `timeout_secs`。
fn default_timeout() -> u64 {
    600
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            account_id: String::new(),
            default_model: CODEX_DEFAULT_MODEL.to_string(),
            base_url: CODEX_BASE_URL.to_string(),
            enable_web_search: true,
            timeout_secs: 600,
            proxy: None,
            stream_mode: String::new(),
        }
    }
}

/// Codex/OpenAI Responses API provider.
pub struct CodexProvider {
    config: CodexConfig,
    client: reqwest::Client,
    token_source: Option<Box<dyn Fn() -> Result<(String, String), String> + Send + Sync>>,
}

impl CodexProvider {
    pub fn new(config: CodexConfig) -> Self {
        let client = build_client(config.timeout_secs, config.proxy.as_deref());
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
        let client = build_client(config.timeout_secs, config.proxy.as_deref());
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
                    instructions = msg.content.to_text();
                }
                "user" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        // D7 同族降级（2026-09-03 二次回归 F2）：function_call_output
                        // 的 output 必须是字符串，Parts 走文本视图 + 诚实注记
                        //（Text 形态返回原串，字节不变；图像进 tool_result 属 Phase 5）。
                        input_items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": tc_id,
                            "output": msg.content.to_prompt_text_with_image_note()
                        }));
                    } else {
                        input_items.push(serde_json::json!({
                            "type": "message",
                            "role": "user",
                            // 该 provider 未实现 vision 数组 → 图像按 D7 诚实降级
                            "content": msg.content.to_prompt_text_with_image_note()
                        }));
                    }
                }
                "assistant" => {
                    if !msg.tool_calls.is_empty() {
                        if !msg.content.is_empty() {
                            input_items.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": msg.content.to_prompt_text_with_image_note()
                            }));
                        }
                        for tc in &msg.tool_calls {
                            let name = tc
                                .name
                                .as_deref()
                                .or_else(|| tc.function.as_ref().map(|f| f.name.as_str()))
                                .unwrap_or("");
                            let args = tc
                                .function
                                .as_ref()
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
                            "content": msg.content.to_prompt_text_with_image_note()
                        }));
                    }
                }
                "tool" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        // D7 同族降级（2026-09-03 二次回归 F2）：function_call_output
                        // 的 output 必须是字符串，Parts 走文本视图 + 诚实注记
                        //（Text 形态返回原串，字节不变；图像进 tool_result 属 Phase 5）。
                        input_items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": tc_id,
                            "output": msg.content.to_prompt_text_with_image_note()
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

        // 流式线缆（2026-10-13 根因修复）：非流式请求要等模型补全完才回响应
        // 头，推理型模型 + 大上下文（工作流对话生成实测 ~150s）必撞中转链路
        // 的 ~120s 墙钟超时 → 连接被掐 → 上游 502 "error sending request"，
        // bot 侧归为 Overloaded 进限流环，重试恒撞同一堵墙永不自愈。Codex
        // 官方客户端恒走流式，号池只伺候流式形态。SSE 首包秒回、事件持续
        // 流出，不受该超时约束。服务端忽略 stream 回整包 JSON 时按
        // content-type 在 chat() 走既有解析路径（字节兼容）。
        // per-model `stream` 键（同日）：默认（auto/空）流式；"no" = 逃生门
        // 强制非流式形态（body 不带 stream 字段，resp.json() 等完整响应）。
        if self.config.stream_mode != "no" {
            body["stream"] = serde_json::json!(true);
        }

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
    let m = if let Some(stripped) = m.strip_prefix("openai/") {
        stripped
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
        "glm",
        "claude",
        "anthropic",
        "gemini",
        "google",
        "moonshot",
        "kimi",
        "qwen",
        "deepseek",
        "llama",
        "meta-llama",
        "mistral",
        "grok",
        "xai",
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
fn translate_tools_for_codex(
    tools: &[ToolDefinition],
    enable_web_search: bool,
) -> Vec<serde_json::Value> {
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
                            if c.get("type").and_then(|t| t.as_str()) == Some("output_text")
                                && let Some(text) = c.get("text").and_then(|t| t.as_str())
                            {
                                content.push_str(text);
                            }
                        }
                    }
                }
                "function_call" => {
                    let id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments_str = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");

                    let arguments: HashMap<String, serde_json::Value> =
                        serde_json::from_str(arguments_str).unwrap_or_else(|_| {
                            let mut m = HashMap::new();
                            m.insert(
                                "raw".to_string(),
                                serde_json::Value::String(arguments_str.to_string()),
                            );
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
            cache_creation_tokens: None,
            cache_read_tokens: None,
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

/// SSE 单事件块的处理结果。
#[derive(Debug, Clone, PartialEq)]
enum SseOutcome {
    /// `response.completed` / `response.incomplete`——携带最终 response 对象
    /// （与非流式响应体同构，可直送 `parse_codex_response`）。
    Finished(serde_json::Value),
    /// `response.failed` / `error`——上游显式失败，带可读原因。
    Failed(String),
    /// 中间事件（response.created / in_progress / output_text.delta 等）继续收。
    Continue,
}

/// 拼接单个 SSE 事件块内的全部 `data:` 行（`event:`/`id:` 行忽略，CRLF 容错），
/// JSON 解析失败或空块返回 None（心跳注释行等静默跳过）。
fn parse_sse_block(block: &str) -> Option<serde_json::Value> {
    let mut data = String::new();
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data).ok()
}

/// 按事件 type 分派（单事件语义抽出便于单测）。
fn handle_sse_event(ev: &serde_json::Value) -> SseOutcome {
    match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "response.completed" | "response.incomplete" => SseOutcome::Finished(
            ev.get("response")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ),
        "response.failed" => {
            let msg = ev
                .pointer("/response/error/message")
                .or_else(|| ev.pointer("/response/status"))
                .and_then(|m| m.as_str())
                .unwrap_or("upstream reported response.failed");
            SseOutcome::Failed(msg.to_string())
        }
        "error" => {
            let msg = ev
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("upstream SSE error event");
            SseOutcome::Failed(msg.to_string())
        }
        _ => SseOutcome::Continue,
    }
}

/// 解析完整 SSE 文本流（聚合模式：找终态事件出结果）。
/// 流自然结束仍无终态事件（中途断流）= 补全不完整，按 Timeout 计——
/// 诚实不可交付，且重试环可安全重抽。
fn parse_sse_text(text: &str, model: &str) -> Result<crate::types::LLMResponse, FailoverError> {
    // 事件分隔符按协议是空行；同时容忍 \n\n 与 \r\n\r\n 两种形态。
    let mut rest = text;
    while !rest.is_empty() {
        let (block, next): (&str, &str) = match (rest.find("\n\n"), rest.find("\r\n\r\n")) {
            (Some(a), Some(b)) => {
                if a <= b {
                    (&rest[..a], &rest[a + 2..])
                } else {
                    (&rest[..b], &rest[b + 4..])
                }
            }
            (Some(a), None) => (&rest[..a], &rest[a + 2..]),
            (None, Some(b)) => (&rest[..b], &rest[b + 4..]),
            (None, None) => (rest, ""),
        };
        rest = next;
        let Some(ev) = parse_sse_block(block) else {
            continue;
        };
        match handle_sse_event(&ev) {
            SseOutcome::Finished(response) => return Ok(parse_codex_response(&response)),
            SseOutcome::Failed(msg) => {
                return Err(FailoverError::Unknown {
                    provider: "codex".to_string(),
                    message: msg,
                });
            }
            SseOutcome::Continue => {}
        }
    }
    Err(FailoverError::Timeout {
        provider: "codex".to_string(),
        model: model.to_string(),
    })
}

/// SSE 聚合解析入口：读完整事件流文本后走 `parse_sse_text`。
/// 本接口（`chat()`）本就等完整补全才返回，增量处理无收益；关键在请求侧
/// `stream: true` 让响应头/事件块持续到达，绕开中转链路的墙钟超时。
async fn parse_sse_response(
    resp: reqwest::Response,
    model: &str,
) -> Result<crate::types::LLMResponse, FailoverError> {
    // 中途断流：客户端超时→Timeout；其余读错误诚实携带根因链（同 send 臂）。
    let text = resp
        .text()
        .await
        .map_err(|e| transport_error("codex", model, &e))?;
    parse_sse_text(&text, model)
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

        let url = format!("{}/responses", self.config.base_url.trim_end_matches('/'));
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
            // 诚实化（2026-10-13）：真超时→Timeout；DNS/连接拒绝/SYN 黑洞等
            // 传输错误→Unknown + "connect error: <根因链>"（此前一律伪装
            // Timeout——21s 恒定超时误导排查两天）。
            .map_err(|e| transport_error("codex", &resolved_model, &e))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            // 先取 Retry-After 头再消费 body（text() 按值拿走 resp）。
            let retry_after = crate::failover::retry_after_from_headers(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            return Err(FailoverError::from_status(
                "codex",
                &resolved_model,
                status,
                &text,
                retry_after,
            ));
        }

        // Content-type 分派（流式线缆 2026-10-13）：SSE 走聚合解析；
        // application/json（服务端忽略 stream 回整包）走既有 json 路径。
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if content_type.contains("text/event-stream") {
            return parse_sse_response(resp, &resolved_model).await;
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
mod cov_tests;
#[cfg(test)]
mod tests;

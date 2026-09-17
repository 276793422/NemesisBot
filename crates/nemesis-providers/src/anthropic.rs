//! Anthropic/Claude provider (Anthropic Messages API).

use crate::failover::FailoverError;
use crate::http_provider::StreamChunk;
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
    /// 出站代理 URL（代理接线修复 2026-09-17）：http/socks5（reqwest
    /// `Proxy::all`）；None = 直连。来自模型条目 `proxy` 字段，经
    /// FactoryConfig 透传。格式非法时 warn 回落直连（与 HttpProvider 同
    /// 策略——启动不能被一个坏代理 URL 杀死）。
    #[serde(default)]
    pub proxy: Option<String>,
}

// P3A 超时对齐（2026-09-12）：120s 曾与 openai 兼容 lane 的 600s 口径分裂
// （超时阶梯「provider 单请求 600s」只对 http lane 成立）——双端真机 S2 评审
// LLM 连续精确 120.01s 超时的根因。全 lane 统一 600s，per-model 可经模型
// 条目 extra `timeout_secs` 覆盖（provider_resolver → FactoryConfig 透传）。
fn default_timeout() -> u64 {
    600
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            timeout_secs: 600,
            proxy: None,
        }
    }
}

/// reqwest client 构造单一出口（timeout + 可选出站代理）。代理 URL 非法
/// 时 warn 回落直连（启动不能被一个坏代理 URL 杀死——与 HttpProvider
/// 同策略）。
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
                    "Anthropic provider: invalid proxy URL, using direct connection"
                );
            }
        }
    }
    builder.build().expect("failed to build HTTP client")
}

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: reqwest::Client,
    token_source: Option<Box<dyn Fn() -> Result<String, String> + Send + Sync>>,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Self {
        let client = build_client(config.timeout_secs, config.proxy.as_deref());
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
        let client = build_client(config.timeout_secs, config.proxy.as_deref());
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
                    // 回归 F1 防御：system 位不支持图像 Parts，raw 序列化会产出
                    // wire 非法块。system 是内部指令位（无"用户附带"语义），
                    // 取纯文本视图即可，与 codex system 分支同语义。
                    system_parts.push(serde_json::json!({
                        "type": "text",
                        "text": msg.content.to_text()
                    }));
                }
                "user" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        // Tool result。D7 同族降级（回归 F1）：tool_result 在本实现
                        // 只承载文本（图像进 tool_result 属 Phase 5），Parts 走
                        // 文本视图 + 诚实注记，防 raw 序列化产出非法 wire 块。
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tc_id,
                                "content": msg.content.to_prompt_text_with_image_note()
                            }]
                        }));
                    } else {
                        // anthropic_content_value：纯文本保持字符串（字节不变）；
                        // Parts 转 blocks（text / image base64|url）。
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": anthropic_content_value(&msg.content)
                        }));
                    }
                }
                "assistant" => {
                    if !msg.tool_calls.is_empty() {
                        let mut content: Vec<serde_json::Value> = Vec::new();
                        if !msg.content.is_empty() {
                            // D7 同族降级（回归 F1）：assistant 不产出图像，Parts
                            // 降级为文本视图 + 诚实注记。
                            content.push(serde_json::json!({
                                "type": "text",
                                "text": msg.content.to_prompt_text_with_image_note()
                            }));
                        }
                        for tc in &msg.tool_calls {
                            let name = tc
                                .name
                                .as_deref()
                                .or_else(|| tc.function.as_ref().map(|f| f.name.as_str()))
                                .unwrap_or("");
                            // 参数单一真相源 = function.arguments（String JSON）：
                            // LlmProvider 桥（llm_bridge::agent_message_to_provider /
                            // CLI agent adapter）只填这一份（arguments: None），
                            // OpenAI lane 的共享序列化同样只发它。此前这里只读
                            // HashMap 版 `tc.arguments` → 桥场景历史 assistant
                            // tool_use input 恒 `{}`，模型下一轮模仿历史空参形态
                            //（UAT U3 glm-5.3-flash「write 后 exec 空参」根因，
                            // 2026-09-15 wire 抓包实锤：响应参数完整、进历史即丢）。
                            // 解析失败/缺失时回退 HashMap 版（部分内部路径直填），
                            // 非 object 结果不采纳（anthropic wire 要求 input 为
                            // object），两者皆缺才 {}。
                            let input = tc
                                .function
                                .as_ref()
                                .and_then(|f| {
                                    serde_json::from_str::<serde_json::Value>(&f.arguments).ok()
                                })
                                .filter(|v| v.is_object())
                                .or_else(|| {
                                    tc.arguments.as_ref().map(|args| {
                                        serde_json::Value::Object(
                                            args.iter()
                                                .map(|(k, v)| (k.clone(), v.clone()))
                                                .collect(),
                                        )
                                    })
                                })
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
                        // D7 同族降级（回归 F1）：同上，Parts 防非法 wire 块。
                        api_messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content.to_prompt_text_with_image_note()
                        }));
                    }
                }
                "tool" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        // D7 同族降级（回归 F1）：同 user tool_result，Parts 走
                        // 文本视图 + 诚实注记（图像进 tool_result 属 Phase 5）。
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tc_id,
                                "content": msg.content.to_prompt_text_with_image_note()
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

        // H4 (U16 half): reasoning effort → Anthropic extended-thinking
        // budget. Anthropic has no "effort" wire field; it takes
        // `thinking: {type: "enabled", budget_tokens: N}`. Tier→budget is a
        // FIXED documented mapping (low=1024 / medium=4096 / high=16384) —
        // budgets must stay well under max_tokens, and these leave headroom
        // under the 4096 default. "off"/empty sends nothing.
        if let Some(ref effort) = options.reasoning_effort {
            let budget = match effort.as_str() {
                "low" => Some(1024usize),
                "medium" => Some(4096usize),
                "high" => Some(16384usize),
                _ => None, // "off" or unknown → no thinking block
            };
            if let Some(b) = budget {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": b
                });
            }
        }

        body
    }

    /// Anthropic Messages API 流式请求（B 根修 2026-09-17）。
    ///
    /// wire：POST {base}/v1/messages + `stream: true`，SSE 事件族
    /// （message_start / content_block_start / content_block_delta /
    /// content_block_stop / message_delta / message_stop / ping / error）。
    /// 映射到 StreamChunk：text_delta → delta；input_json_delta 按 index
    /// 累积、content_block_stop 定稿；message_delta.stop_reason 归一化为
    /// OpenAI 风格 finish_reason（tool_use→tool_calls / max_tokens→length /
    /// 其余→stop），随 message_stop 发终态 chunk 并按 block 顺序 flush
    /// tool_calls；usage = message_start 的 input_tokens（含 cache 维度）+
    /// message_delta 的 output_tokens。EOF 无 message_stop 合成终态 chunk
    ///（对齐 HttpProvider 语义）；流读错误 → Timeout（可 failover，J1 同族）；
    /// error 事件 → Format（overloaded_error → Overloaded）。
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

        let api_key = match self.get_api_key() {
            Ok(k) => k,
            Err(e) => {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                let _ = tx.try_send(Err(e));
                return rx;
            }
        };

        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));
        let mut body = self.build_request_body(messages, tools, &model, options);
        body["stream"] = serde_json::json!(true);

        let client = self.client.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let resp = match client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(_) => {
                    let _ = tx
                        .send(Err(FailoverError::Timeout {
                            provider: "anthropic".to_string(),
                            model: model.clone(),
                        }))
                        .await;
                    return;
                }
            };

            let status = resp.status().as_u16();
            if status >= 400 {
                // 先取 Retry-After 头再消费 body（text() 按值拿走 resp）。
                let retry_after = crate::failover::retry_after_from_headers(resp.headers());
                let text = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(FailoverError::from_status(
                        "anthropic",
                        &model,
                        status,
                        &text,
                        retry_after,
                    )))
                    .await;
                return;
            }

            // 200 但显式 application/json：不是 SSE 流。有 content 数组 =
            // 服务端忽略 stream:true 回了完整消息（兼容行为）→ parse_response
            // 合成流；否则亮出 body 真相（对齐 HttpProvider 的 200 守卫）。
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if content_type.contains("application/json") {
                let raw = resp.text().await.unwrap_or_default();
                match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(v)
                        if v.get("content")
                            .and_then(|c| c.as_array())
                            .is_some_and(|a| !a.is_empty()) =>
                    {
                        let parsed = parse_response(&v);
                        if !parsed.content.is_empty() {
                            let _ = tx
                                .send(Ok(StreamChunk {
                                    delta: parsed.content.clone(),
                                    tool_calls: vec![],
                                    finish_reason: None,
                                    usage: None,
                                    reasoning_content: None,
                                }))
                                .await;
                        }
                        let _ = tx
                            .send(Ok(StreamChunk {
                                delta: String::new(),
                                tool_calls: parsed.tool_calls,
                                finish_reason: Some(parsed.finish_reason),
                                usage: parsed.usage,
                                reasoning_content: None,
                            }))
                            .await;
                    }
                    Ok(_) => {
                        let _ = tx
                            .send(Err(FailoverError::Format {
                                provider: "anthropic".to_string(),
                                message: format!(
                                    "expected text/event-stream for model {}, got JSON body: {}",
                                    model,
                                    raw.chars().take(200).collect::<String>()
                                ),
                            }))
                            .await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(FailoverError::Format {
                                provider: "anthropic".to_string(),
                                message: format!("non-SSE JSON body (model {}): {}", model, e),
                            }))
                            .await;
                    }
                }
                return;
            }

            // SSE 解析。anthropic 每事件一个 data: 行（JSON 内带 type 字段，
            // event: 行冗余不依赖）。
            let mut stream = resp.bytes_stream();
            use futures::StreamExt;
            let mut buffer = String::new();
            // index -> (tool_use id, name, 累积 partial_json)
            let mut pending_tools: HashMap<usize, (String, String, String)> = HashMap::new();
            // content_block 出现顺序（终态按序 flush；HashMap 无序）
            let mut tool_order: Vec<usize> = Vec::new();
            let mut input_tokens: Option<i64> = None;
            let mut cache_creation: Option<i64> = None;
            let mut cache_read: Option<i64> = None;
            let mut output_tokens: Option<i64> = None;
            let mut stop_reason: Option<String> = None;
            let mut accumulated_reasoning = String::new();

            while let Some(chunk_result) = stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(
                            provider = "anthropic",
                            error = %e,
                            "[Provider] Anthropic SSE stream read error"
                        );
                        let _ = tx
                            .send(Err(FailoverError::Timeout {
                                provider: "anthropic".to_string(),
                                model: model.clone(),
                            }))
                            .await;
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(pos) = buffer.find("\n\n") {
                    let block = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    // 拼接 block 内全部 data: 行（稳妥；anthropic 实际单行）。
                    let mut data = String::new();
                    for line in block.lines() {
                        let line = line.trim();
                        if let Some(rest) = line.strip_prefix("data: ") {
                            data.push_str(rest.trim());
                        } else if let Some(rest) = line.strip_prefix("data:") {
                            data.push_str(rest.trim());
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    let parsed: serde_json::Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let ev_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    match ev_type {
                        "message_start" => {
                            let u = parsed.pointer("/message/usage");
                            input_tokens = u
                                .and_then(|u| u.get("input_tokens"))
                                .and_then(|v| v.as_i64());
                            cache_creation = u
                                .and_then(|u| u.get("cache_creation_input_tokens"))
                                .and_then(|v| v.as_i64());
                            cache_read = u
                                .and_then(|u| u.get("cache_read_input_tokens"))
                                .and_then(|v| v.as_i64());
                        }
                        "content_block_start" => {
                            let idx =
                                parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            let block = parsed.get("content_block").cloned();
                            if block
                                .as_ref()
                                .and_then(|b| b.get("type"))
                                .and_then(|v| v.as_str())
                                == Some("tool_use")
                            {
                                let b = block.unwrap_or_default();
                                let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                if !tool_order.contains(&idx) {
                                    tool_order.push(idx);
                                }
                                pending_tools.entry(idx).or_insert_with(|| {
                                    (id.to_string(), name.to_string(), String::new())
                                });
                            }
                        }
                        "content_block_delta" => {
                            let idx =
                                parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            let delta = parsed.get("delta").cloned().unwrap_or_default();
                            match delta.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                                "text_delta" => {
                                    let text =
                                        delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                    if !text.is_empty() {
                                        let chunk = StreamChunk {
                                            delta: text.to_string(),
                                            tool_calls: vec![],
                                            finish_reason: None,
                                            usage: None,
                                            reasoning_content: None,
                                        };
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(pj) =
                                        delta.get("partial_json").and_then(|v| v.as_str())
                                        && let Some(entry) = pending_tools.get_mut(&idx)
                                    {
                                        entry.2.push_str(pj);
                                    }
                                }
                                "thinking_delta" => {
                                    if let Some(th) = delta.get("thinking").and_then(|v| v.as_str())
                                    {
                                        accumulated_reasoning.push_str(th);
                                    }
                                }
                                _ => {}
                            }
                        }
                        "message_delta" => {
                            if let Some(sr) = parsed
                                .pointer("/delta/stop_reason")
                                .and_then(|v| v.as_str())
                            {
                                stop_reason = Some(sr.to_string());
                            }
                            if let Some(ot) = parsed
                                .pointer("/usage/output_tokens")
                                .and_then(|v| v.as_i64())
                            {
                                output_tokens = Some(ot);
                            }
                        }
                        "message_stop" => {
                            let chunk = anthropic_final_chunk(
                                stop_reason.as_deref(),
                                &tool_order,
                                &pending_tools,
                                input_tokens,
                                output_tokens,
                                cache_creation,
                                cache_read,
                                if accumulated_reasoning.is_empty() {
                                    None
                                } else {
                                    Some(accumulated_reasoning.clone())
                                },
                            );
                            let _ = tx.send(Ok(chunk)).await;
                            return;
                        }
                        "error" => {
                            let etype = parsed
                                .pointer("/error/type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let msg = parsed
                                .pointer("/error/message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("stream error");
                            let err = if etype == "overloaded_error" {
                                FailoverError::Overloaded {
                                    provider: "anthropic".to_string(),
                                }
                            } else {
                                FailoverError::Format {
                                    provider: "anthropic".to_string(),
                                    message: format!("{}: {}", etype, msg),
                                }
                            };
                            let _ = tx.send(Err(err)).await;
                            return;
                        }
                        // ping / content_block_stop / 未知事件：不产 chunk。
                        _ => {}
                    }
                }
            }

            // EOF 无 message_stop（半截流）——合成终态 chunk，对齐 HttpProvider
            // 的 EOF 语义（不悬挂 receiver，可能时 flush 已累积 tool_calls）。
            tracing::warn!(
                provider = "anthropic",
                model = %model,
                tool_call_count = tool_order.len(),
                "[Provider] Anthropic SSE stream ended without message_stop — synthesized termination chunk"
            );
            let chunk = anthropic_final_chunk(
                stop_reason.as_deref(),
                &tool_order,
                &pending_tools,
                input_tokens,
                output_tokens,
                cache_creation,
                cache_read,
                if accumulated_reasoning.is_empty() {
                    None
                } else {
                    Some(accumulated_reasoning)
                },
            );
            let _ = tx.send(Ok(chunk)).await;
        });

        rx
    }
}

/// anthropic 流式终态 chunk：finish_reason 归一化（tool_use→tool_calls /
/// max_tokens→length / 其余→stop；reason 缺失但有累积 tool → tool_calls）+
/// 按 content_block 顺序 flush tool_calls（arguments 为累积 partial_json，
/// 空 → "{}"）+ usage 合成（input + output，含 cache 维度）。
fn anthropic_final_chunk(
    stop_reason: Option<&str>,
    tool_order: &[usize],
    pending: &HashMap<usize, (String, String, String)>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation: Option<i64>,
    cache_read: Option<i64>,
    reasoning: Option<String>,
) -> StreamChunk {
    let tool_calls: Vec<ToolCall> = tool_order
        .iter()
        .filter_map(|i| {
            let (id, name, args_json) = pending.get(i)?;
            let args = if args_json.trim().is_empty() {
                "{}".to_string()
            } else {
                args_json.clone()
            };
            Some(ToolCall {
                id: id.clone(),
                call_type: Some("tool_use".to_string()),
                function: Some(FunctionCall {
                    name: name.clone(),
                    arguments: args,
                }),
                name: Some(name.clone()),
                arguments: None,
            })
        })
        .collect();

    let finish_reason = match stop_reason.unwrap_or("") {
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "" if !tool_calls.is_empty() => "tool_calls",
        _ => "stop",
    }
    .to_string();

    let usage = if input_tokens.is_some() || output_tokens.is_some() {
        let prompt = input_tokens.unwrap_or(0);
        let completion = output_tokens.unwrap_or(0);
        Some(UsageInfo {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_tokens: None,
            cache_creation_tokens: cache_creation,
            cache_read_tokens: cache_read,
        })
    } else {
        None
    };

    StreamChunk {
        delta: String::new(),
        tool_calls,
        finish_reason: Some(finish_reason),
        usage,
        reasoning_content: reasoning,
    }
}

/// 单条消息 content → Anthropic Messages API content 值：
/// - `Text`  → JSON 字符串（字节兼容现网请求，prompt cache 前缀不变）
/// - `Parts` → blocks 数组：Text → `{"type":"text",text}`；
///   Base64 图 → `{"type":"image","source":{"type":"base64",media_type,data}}`；
///   Url 图 → `{"type":"image","source":{"type":"url",url}}`。
///   （Anthropic image block 无 detail 字段——D2 透传仅 OpenAI 形态适用。）
fn anthropic_content_value(content: &crate::types::MessageContent) -> serde_json::Value {
    match content {
        crate::types::MessageContent::Text(_) => {
            serde_json::to_value(content).expect("MessageContent serialize is infallible")
        }
        crate::types::MessageContent::Parts(parts) => serde_json::Value::Array(
            parts
                .iter()
                .map(|p| match p {
                    crate::types::ContentPart::Text { text } => {
                        serde_json::json!({ "type": "text", "text": text })
                    }
                    crate::types::ContentPart::Image { image, .. } => match image {
                        crate::types::ImageSource::Base64 { media_type, data } => {
                            serde_json::json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": data
                                }
                            })
                        }
                        crate::types::ImageSource::Url(url) => {
                            serde_json::json!({
                                "type": "image",
                                "source": { "type": "url", "url": url }
                            })
                        }
                    },
                })
                .collect(),
        ),
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
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
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
        _ => "stop",
    };

    let usage = if let Some(u) = data.get("usage") {
        let prompt = u.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let completion = u.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let cache_creation = u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_i64());
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

        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));
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
            // 先取 Retry-After 头再消费 body（text() 按值拿走 resp）。
            let retry_after = crate::failover::retry_after_from_headers(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            return Err(FailoverError::from_status(
                "anthropic",
                model,
                status,
                &text,
                retry_after,
            ));
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| FailoverError::Format {
            provider: "anthropic".to_string(),
            message: e.to_string(),
        })?;

        Ok(parse_response(&data))
    }

    fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        // trait 投影（B 根修 2026-09-17）：trait 默认实现是「不支持」，anthropic
        // lane 在此接通真流式（此前该协议整条 lane 无流式能力）。完全限定调用
        // 解析到本类型固有 impl。
        AnthropicProvider::chat_stream(self, messages, tools, model, options)
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

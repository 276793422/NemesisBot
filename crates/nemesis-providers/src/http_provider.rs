//! HTTP-based LLM provider (OpenAI-compatible).

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Extract UsageInfo from a usage JSON value, including cached token metrics.
///
/// 账本不变量：`prompt_tokens` 归一为**非缓存纯输入**（Anthropic 语义），
/// 缓存读单列 `cache_read_tokens`。OpenAI/DeepSeek 族 wire 的 prompt/input
/// 均含缓存（子集语义），此处统一剥出；miss 段就是普通输入——OpenAI 族
/// 没有缓存写概念，恒不落 `cache_creation_tokens`（Anthropic 族除外）。
fn extract_usage(u: &serde_json::Value) -> UsageInfo {
    let prompt_raw = u["prompt_tokens"].as_i64().unwrap_or(0);

    // DeepSeek: prompt_cache_hit_tokens + prompt_cache_miss_tokens
    // （miss 段可由 prompt - hit 推导，不单独读取）
    let ds_hit = u.get("prompt_cache_hit_tokens").and_then(|v| v.as_i64());

    // OpenAI: usage.prompt_tokens_details.cached_tokens
    let oai_cached = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_i64());

    // Anthropic: cache_creation_input_tokens + cache_read_input_tokens
    let ant_creation = u
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64());
    let ant_read = u.get("cache_read_input_tokens").and_then(|v| v.as_i64());

    let (plain_prompt, cached, creation, read) = if let Some(hit) = ds_hit {
        // DeepSeek: prompt = hit + miss → 剥出 hit；miss 归普通输入。
        let hit = hit.clamp(0, prompt_raw.max(0));
        (prompt_raw - hit, Some(hit), None, Some(hit))
    } else if let Some(c) = oai_cached {
        // OpenAI: prompt_tokens 含 cached_tokens（子集）→ 同上剥出。
        let c = c.clamp(0, prompt_raw.max(0));
        (prompt_raw - c, Some(c), None, Some(c))
    } else if ant_creation.is_some() || ant_read.is_some() {
        // Anthropic: prompt_tokens 原生即非缓存纯输入，不动；读写单列。
        (prompt_raw, None, ant_creation, ant_read)
    } else {
        (prompt_raw, None, None, None)
    };

    UsageInfo {
        prompt_tokens: plain_prompt,
        completion_tokens: u["completion_tokens"].as_i64().unwrap_or(0),
        total_tokens: u["total_tokens"].as_i64().unwrap_or(0),
        cached_tokens: cached,
        cache_creation_tokens: creation,
        cache_read_tokens: read,
    }
}

/// 块内全部 data: 行载荷（按行保序）。SSE 规范 `data` 字段名后的空格
/// 可选——两种形态都识别；此前只认 `"data: "` 带空格形态，无空格上游
/// （合规服务存在）事件全被跳过 → 流式回复静默变空。
fn data_payloads(block: &str) -> Vec<&str> {
    block
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
        })
        .collect()
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
    /// per-model 流式开关（2026-10-13 stream 键）：空（auto）= 维持非流式
    /// （现状）；"yes" = `chat()` 内部走流式（聚合固有 `chat_stream`，对外
    /// 签名不变）——非流式请求等完整补全才回响应头，推理型模型 + 大上下文
    /// 会撞中转链路 ~120s 墙钟（gpt-5.6-sol 风暴根因，openai 兼容 lane 同墙）。
    /// 经 FactoryConfig.stream_mode 透传，构造期消费。
    #[serde(default)]
    pub stream_mode: String,
}

// P3A 超时对齐（2026-09-12）：serde 缺省与 factory 显式值统一 600s
// （原 120s——评审 LLM 连续 120s 超时根因的 http 分支）。
fn default_timeout() -> u64 {
    600
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
        let mut builder =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(config.timeout_secs));

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

        // 与 openai_compat 同一转换（crate 共享 helper）：纯文本 content
        // 保持字符串（字节不变）；Parts 消息转 OpenAI vision 数组。
        let mut body = serde_json::json!({
            "model": normalized,
            "messages": crate::types::messages_to_openai_json(messages),
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

        // (Removed) Previously forced `thinking: {type: "disabled"}` for deepseek
        // + tools. That put deepseek-v4-flash in an inconsistent state: it stopped
        // returning `reasoning_content`, yet the API still requires `reasoning_content`
        // to be passed back in multi-turn history → HTTP 400 ("The reasoning_content
        // in the thinking mode must be passed back") on the 2nd turn. With thinking
        // left on, the response carries `reasoning_content` (extracted at the parse
        // site below), stored on the turn and replayed — correct for thinking models.

        body
    }

    /// Send a streaming chat completion request.
    ///
    /// Sets `stream: true` in the request body and returns a channel receiver
    /// that yields `StreamChunk`s as they arrive from the LLM API.
    ///
    /// The returned `tokio::sync::mpsc::Receiver` will be closed when the
    /// stream ends (either naturally on `[DONE]` or on error).
    ///
    /// J1 流式边界：
    /// - HTTP 错误（status≥400）携带 `Retry-After` 头解析出的秒数（failover
    ///   侧冷却据此优先于公式退避）。
    /// - SSE 读错误分类为 `Timeout`（可 failover）而非 `Format`（不可重试）——
    ///   连接中断是暂时性故障，换 provider/重试是正确反应。
    /// - EOF 无 `[DONE]`（半截响应）：flush 已累积的 tool_calls + 发一个合成
    ///   终止 chunk（finish_reason 有 tool_calls 时为 `tool_calls`，否则
    ///   `stop`）——receiver 消费者看到唯一 final chunk，不再悬挂等待。
    ///   内容可能不完整，整请求级重试由调用方（agent loop 的 context-error
    ///   压缩重试路径）负责，provider 层不重放半截流。
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

        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut body = self.build_request_body(messages, tools, &model, options);
        body["stream"] = serde_json::json!(true);
        // include_usage（2026-10-23 根修）：OpenAI 语义下流式 usage 只在请求
        // 带 stream_options.include_usage 时出现——终态 finish chunk 之后、
        // choices:[] 的专用 usage chunk。此前既不请求也不转发，stream=yes
        // 模型在严格 OpenAI 形态端点上账本恒按 0 记（usage 面板/审计/
        // board token 预算保险丝漏记）；usage 内嵌 finish chunk 的端点
        // （DeepSeek 等）不受影响。主流兼容实现均支持该键；个别严格端点
        // 若拒绝，诚实 400 走故障转移，好过静默零账。
        body["stream_options"] = serde_json::json!({ "include_usage": true });

        let api_key = self.config.api_key.clone();
        let headers = self.config.headers.clone();
        let client = self.client.clone();
        let provider_name = self.config.name.clone();
        let stream_timeout_secs = self.config.timeout_secs;

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
                Err(e) => {
                    // 诚实化（2026-10-13）：真超时→Timeout；其余传输错误→
                    // Unknown + "connect error: <根因链>"（不伪装）。
                    let _ = tx
                        .send(Err(crate::failover::transport_error(
                            &provider_name,
                            &model,
                            &e,
                        )))
                        .await;
                    return;
                }
            };

            let status = resp.status().as_u16();
            if status >= 400 {
                // 先取 Retry-After 头再消费 body（text() 按值拿走 resp）。
                let retry_after = crate::failover::retry_after_from_headers(resp.headers());
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
                        retry_after,
                    )))
                    .await;
                return;
            }

            // 200-包装错误诚实化（同 chat() 根修，2026-09-17）：显式
            // application/json 的 200 响应不是 SSE 流——两种可能：
            // ① 网关错误包装体（CC Switch 实测 {"code":500,"msg":"404
            //    NOT_FOUND"}，旧逻辑静默产出空流）→ Format 错误带 body 摘要；
            // ② 服务端忽略 stream:true 回了完整 JSON（兼容行为）→ 合成
            //    单 delta + 终态 chunk，等价流式消费。
            // 其余 content-type（含未标注，如部分 mock）走原 SSE 解析不变。
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if content_type.contains("application/json") {
                let body = resp.text().await.unwrap_or_default();
                let parsed: serde_json::Value = match serde_json::from_str(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx
                            .send(Err(FailoverError::Format {
                                provider: provider_name.clone(),
                                message: format!("non-SSE JSON body (model {}): {}", model, e),
                            }))
                            .await;
                        return;
                    }
                };
                let has_choices = parsed
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| !a.is_empty());
                if has_choices {
                    // ② 忽略 stream 的兼容端点：完整 JSON → 合成流。
                    let content = parsed["choices"][0]["message"]["content"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let finish = parsed["choices"][0]["finish_reason"]
                        .as_str()
                        .unwrap_or("stop")
                        .to_string();
                    // 非流式完整形态的 message.tool_calls 原样带上（2026-10-13
                    // wal_crash 实证修）：工具调用模型在此路径此前被丢弃成
                    // tool_calls=[]，agent 侧只见空文本终答。anthropic 同路径
                    // （parse_response）一直保留，此处对齐。
                    let msg_tool_calls: Vec<ToolCall> =
                        parsed["choices"][0]["message"]["tool_calls"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|tc| {
                                        Some(ToolCall {
                                            id: tc["id"].as_str()?.to_string(),
                                            call_type: Some(
                                                tc["type"]
                                                    .as_str()
                                                    .unwrap_or("function")
                                                    .to_string(),
                                            ),
                                            function: Some(FunctionCall {
                                                name: tc["function"]["name"]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .to_string(),
                                                arguments: tc["function"]["arguments"]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .to_string(),
                                            }),
                                            name: None,
                                            arguments: None,
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                    let usage = parsed.get("usage").map(extract_usage);
                    if !content.is_empty() {
                        let _ = tx
                            .send(Ok(StreamChunk {
                                delta: content,
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
                            tool_calls: msg_tool_calls,
                            finish_reason: Some(finish),
                            usage,
                            reasoning_content: None,
                        }))
                        .await;
                } else {
                    // ① 错误包装体（或异常形态）——亮出 body 真相。
                    tracing::error!(
                        provider = %provider_name,
                        model = %model,
                        "[Provider] Streaming request got non-SSE JSON body"
                    );
                    let _ = tx
                        .send(Err(FailoverError::Format {
                            provider: provider_name.clone(),
                            message: format!(
                                "expected text/event-stream for model {}, got JSON body: {}",
                                model,
                                body.chars().take(200).collect::<String>()
                            ),
                        }))
                        .await;
                }
                return;
            }

            // Parse the SSE stream from the response body.
            let mut stream = resp.bytes_stream();
            // 墙钟约束（2026-10-16 NB-123 实证根修）：reqwest client 级
            // `.timeout()` 在 `bytes_stream()` 读路径不兜底（0.12.28 实测：
            // 上游静默停流时整个任务在 `stream.next()` 挂死，1800s 配置形同
            // 虚设，直到 B 端 2h 兜底才解）。以「请求起点 + timeout_secs」为
            // 绝对墙钟线，SSE 每次读都在线内——到线即 Timeout（可重试/可
            // 故障转移），超时阶梯「provider 单请求墙钟」在流式 lane 真正
            // 落地。timeout_secs == 0 = 不限（显式逃生口）。
            let deadline = if stream_timeout_secs == 0 {
                None
            } else {
                Some(
                    tokio::time::Instant::now()
                        + std::time::Duration::from_secs(stream_timeout_secs),
                )
            };
            let mut buffer = String::new();
            // 增量 UTF-8 解码残尾（chunk 边界劈开的多字节字符，BUG-HUNT R38）。
            let mut utf8_pending: Vec<u8> = Vec::new();
            // Accumulated tool calls across chunks (index -> partial ToolCall).
            let mut pending_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();
            // Accumulated reasoning content from thinking-mode models.
            let mut accumulated_reasoning = String::new();
            // 终态帧是否已发（finish chunk 正常先行，[DONE] 只收尾不再补发；
            // 2026-10-13 wal_crash 实证根修——[DONE] 硬编码 "stop" 二连发会把
            // 聚合侧已收的 finish_reason="tool_calls" 覆盖成 "stop"，工具调用
            // 轮被判成空终答）。
            let mut finish_emitted = false;

            loop {
                let next = match deadline {
                    Some(dl) => match tokio::time::timeout_at(dl, stream.next()).await {
                        Ok(v) => v,
                        Err(_) => {
                            tracing::error!(
                                provider = %provider_name,
                                model = %model,
                                timeout_secs = stream_timeout_secs,
                                "[Provider] SSE stream exceeded total wall-clock deadline"
                            );
                            let _ = tx
                                .send(Err(FailoverError::Timeout {
                                    provider: provider_name.clone(),
                                    model: model.clone(),
                                }))
                                .await;
                            return;
                        }
                    },
                    None => stream.next().await,
                };
                let Some(chunk_result) = next else {
                    break;
                };
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(
                            provider = %provider_name,
                            error = %e,
                            "[Provider] SSE stream read error"
                        );
                        // J1：读错误（连接中断）是暂时性故障 → 可 failover；
                        // 诚实化（2026-10-13）：客户端超时→Timeout，其余读
                        // 错误→Unknown + "connect error: <根因链>"。旧分类
                        // Format 会直接放弃整条链。终止后 return（不走下方
                        // EOF flush——错误已是终态，不再发合成 chunk 混淆
                        // 消费者）。
                        let _ = tx
                            .send(Err(crate::failover::transport_error(
                                &provider_name,
                                &model,
                                &e,
                            )))
                            .await;
                        return;
                    }
                };

                // 增量解码（R38）：劈在 chunk 边界上的多字节字符先 holdback，
                // 不逐 chunk 独立 lossy（那会把半个字符换成 U+FFFD）。
                crate::stream_util::push_chunk_incremental(&mut buffer, &mut utf8_pending, &bytes);

                // Process complete SSE lines. 事件边界双形态（\n\n / \r\n\r\n，
                // SSE 规范行终止三态；CRLF-only 上游按单一 \n\n 扫描零事件）。
                while let Some((pos, sep)) = crate::stream_util::next_event_boundary(&buffer) {
                    let block = buffer[..pos].to_string();
                    buffer = buffer[pos + sep..].to_string();

                    for data in data_payloads(&block) {
                        if data.trim() == "[DONE]" {
                            if finish_emitted {
                                // 正常收尾（OpenAI 语义 finish → [DONE]）：终态
                                // 帧已随 finish chunk 发出，这里只关闭通道。与
                                // EOF 路径同一条纪律——final chunk 单发，二连发
                                // 会让覆盖式消费的下游丢 tool_calls。
                                tracing::debug!(
                                    provider = %provider_name,
                                    model = %model,
                                    "[Provider] SSE stream completed ([DONE]) — final chunk already emitted"
                                );
                                return;
                            }
                            // 异常形态：[DONE] 前没有 finish 帧——合成唯一终态
                            // （flush 已累积 tool_calls），语义对齐 EOF 路径。
                            let done_calls: Vec<ToolCall> = pending_tool_calls
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
                                .collect();
                            tracing::debug!(
                                provider = %provider_name,
                                model = %model,
                                "[Provider] SSE [DONE] without finish chunk — synthesizing termination chunk"
                            );
                            let _ = tx
                                .send(Ok(StreamChunk {
                                    delta: String::new(),
                                    tool_calls: done_calls,
                                    finish_reason: Some(
                                        if !pending_tool_calls.is_empty() {
                                            "tool_calls"
                                        } else {
                                            "stop"
                                        }
                                        .to_string(),
                                    ),
                                    usage: None,
                                    reasoning_content: if accumulated_reasoning.is_empty() {
                                        None
                                    } else {
                                        Some(accumulated_reasoning.clone())
                                    },
                                }))
                                .await;
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
                        if let Some(rc) =
                            parsed["choices"][0]["delta"]["reasoning_content"].as_str()
                        {
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
                                if let Some(name) = tc["function"]["name"].as_str() {
                                    entry.1 = name.to_string();
                                }
                                if let Some(args) = tc["function"]["arguments"].as_str() {
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
                            // 终态后 usage 专用 chunk（include_usage 形态：
                            // choices 空数组 + usage，finish 已随前一帧发出）
                            // ——转发 usage 给覆盖式聚合侧（stream_util 出现
                            // 即覆盖收下）；其余无内容 chunk 照旧丢弃。不碰
                            // finish_emitted（终态已发，[DONE]/EOF 收尾不变）。
                            if parsed["choices"].as_array().is_some_and(|a| a.is_empty())
                                && let Some(usage) = parsed.get("usage").map(extract_usage)
                            {
                                let chunk = StreamChunk {
                                    delta: String::new(),
                                    tool_calls: vec![],
                                    finish_reason: None,
                                    usage: Some(usage),
                                    reasoning_content: None,
                                };
                                if tx.send(Ok(chunk)).await.is_err() {
                                    return;
                                }
                            }
                            continue;
                        }

                        let usage = if finish_reason.is_some() {
                            parsed.get("usage").map(extract_usage)
                        } else {
                            None
                        };

                        let chunk = StreamChunk {
                            delta: delta_content,
                            tool_calls,
                            finish_reason: finish_reason.clone(),
                            usage,
                            reasoning_content: if finish_reason.is_some()
                                && !accumulated_reasoning.is_empty()
                            {
                                Some(accumulated_reasoning.clone())
                            } else {
                                None
                            },
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            // Receiver dropped, stop streaming.
                            return;
                        }
                        if finish_reason.is_some() {
                            finish_emitted = true;
                        }
                    }
                }
            }

            // J1：EOF 无 [DONE]（半截响应）——不悬挂 receiver。合成唯一
            // final chunk：有累积 tool_calls 时一并 flush（finish_reason=
            // "tool_calls"，OpenAI 语义），否则纯 "stop"。单 chunk 而非
            // tool_calls chunk + stop chunk 两连发——覆盖式消费 final chunk
            // 的下游会在第二发丢掉 tool_calls。
            // 终态帧已发（finish chunk 正常收尾后服务端直接断流）→ 不再补发：
            // 二连发会让聚合侧把已收的 "tool_calls" 覆盖成 "stop"（2026-10-13
            // wal_crash 实证根修）。
            if finish_emitted {
                tracing::debug!(
                    provider = %provider_name,
                    model = %model,
                    "[Provider] SSE stream ended after final chunk — no synthesized duplicate"
                );
                return;
            }
            let flushed_calls: Vec<ToolCall> = pending_tool_calls
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
                .collect();
            tracing::warn!(
                provider = %provider_name,
                model = %model,
                tool_call_count = flushed_calls.len(),
                "[Provider] SSE stream ended without [DONE] — synthesized termination chunk"
            );
            let _ = tx
                .send(Ok(StreamChunk {
                    delta: String::new(),
                    finish_reason: Some(
                        if !flushed_calls.is_empty() {
                            "tool_calls"
                        } else {
                            "stop"
                        }
                        .to_string(),
                    ),
                    tool_calls: flushed_calls,
                    usage: None,
                    reasoning_content: if accumulated_reasoning.is_empty() {
                        None
                    } else {
                        Some(accumulated_reasoning.clone())
                    },
                }))
                .await;
        });

        rx
    }
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// A chunk of streamed LLM response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[async_trait]
impl LLMProvider for HttpProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        // per-model `stream` 键（2026-10-13）：`yes` = 内部流式化——复用固有
        // `chat_stream`（SSE 解析/tool_calls 累积/usage 全同源）聚合为非流式
        // `LLMResponse`，`chat()` 对调用方签名不变。语义对齐 protocol：构造期
        // 消费，改条目需重新设默认或重启。auto（缺省）维持非流式现状。
        if self.config.stream_mode == "yes" {
            return crate::stream_util::collect_stream_response(
                &self.config.name,
                model,
                HttpProvider::chat_stream(self, messages, tools, model, options),
            )
            .await;
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

        tracing::debug!(
            provider = %self.config.name,
            model = %model,
            message_count = messages.len(),
            tool_count = tools.len(),
            "[Provider] Sending non-streaming request"
        );

        let mut req = self
            .client
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
            // 诚实化（2026-10-13）：真超时→Timeout；DNS/连接拒绝/SYN 黑洞等
            // 传输错误→Unknown + "connect error: <根因链>"（此前一律伪装
            // Timeout，连接失败与上游慢处置不同却不可区分）。
            .map_err(|e| crate::failover::transport_error(&self.config.name, model, &e))?;

        let status = resp.status().as_u16();

        if status >= 400 {
            // 先取 Retry-After 头再消费 body（text() 按值拿走 resp）。
            let retry_after = crate::failover::retry_after_from_headers(resp.headers());
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
                retry_after,
            ));
        }

        let raw_response_text = resp.text().await.map_err(|e| FailoverError::Format {
            provider: self.config.name.clone(),
            message: e.to_string(),
        })?;
        let data: serde_json::Value =
            serde_json::from_str(&raw_response_text).map_err(|e| FailoverError::Format {
                provider: self.config.name.clone(),
                message: e.to_string(),
            })?;

        // 200-包装错误诚实化（2026-09-17 根修附带）：部分网关（实测 CC Switch）
        // 对未知路径回 HTTP 200 + 错误 JSON（如 {"code":500,"msg":"404 NOT_FOUND"}）。
        // 旧逻辑 choices 缺失时 unwrap_or("") → 空响应静默成功，上游真相被吞
        //（集群人格生成 0.4s 假失败根因链的一环）。现要求响应体含非空 choices
        // 数组；OpenAI wire 的合法响应至少 1 条 choice，空/缺失即报 Format 并
        // 带 body 摘要，把网关的 200 包装错误亮出来。
        if data.get("error").is_some_and(|e| !e.is_null()) {
            return Err(FailoverError::Format {
                provider: self.config.name.clone(),
                message: format!(
                    "error body with HTTP 200: {}",
                    raw_response_text.chars().take(200).collect::<String>()
                ),
            });
        }
        let choices_absent = data
            .get("choices")
            .and_then(|c| c.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true);
        if choices_absent {
            return Err(FailoverError::Format {
                provider: self.config.name.clone(),
                message: format!(
                    "HTTP 200 but no choices in response body: {}",
                    raw_response_text.chars().take(200).collect::<String>()
                ),
            });
        }

        // Parse OpenAI-compatible response
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let mut finish_reason = data["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("stop")
            .to_string();

        let usage = data.get("usage").map(extract_usage);

        let mut tool_calls =
            if let Some(tc_array) = data["choices"][0]["message"]["tool_calls"].as_array() {
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

        // Tool-call repair: some models (especially smaller local models) emit
        // tool calls as plain text in `content` (DSML / JSON / XML) instead of the
        // standard `tool_calls` structure. When the standard field is empty,
        // recover tool calls from the content so the agent loop still runs them.
        // 白名单过滤（BUG-20261025-16）：启发式对 `name` 键零校验，文档性
        // JSON（```json 配置示例等）会被劫持成幻影工具调用 + finish_reason
        // 强改 "tool_calls"。恢复出的名字必须命中本请求的 tools 白名单
        // （不在白名单的工具无论如何执行不了）；全滤空 = 维持原 finish，
        // 宁可不修复不可误修复。
        if tool_calls.is_empty() && !content.is_empty() {
            let repaired = crate::tool_call_repair::repair_tool_calls_in_whitelist(&content, tools);
            if !repaired.is_empty() {
                tracing::info!(
                    provider = %self.config.name,
                    model = model,
                    repaired_count = repaired.len(),
                    "[Provider] tool-call repair: recovered tool call(s) from content"
                );
                tool_calls = repaired;
                finish_reason = "tool_calls".to_string();
            }
        }

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

    fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        // trait 投影（B 根修 2026-09-17）：trait 默认实现是「不支持」，OpenAI
        // wire lane 在此接通真流式。完全限定调用解析到本类型固有 impl（固有
        // 方法优先于 trait 方法）。
        HttpProvider::chat_stream(self, messages, tools, model, options)
    }

    fn stream_preferred(&self, _model: &str) -> bool {
        // 与 chat() 的内部切换同判据（767 行）：openai 兼容 lane auto =
        // 非流式（2026-10-13 stream 键语义），显式 yes 才流式。
        self.config.stream_mode == "yes"
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

#[cfg(test)]
mod http_provider_extra_tests;

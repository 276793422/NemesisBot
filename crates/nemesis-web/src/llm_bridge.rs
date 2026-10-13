//! LLM bridge types — adapt `nemesis_providers::LLMProvider` for downstream consumers.
//!
//! Two adapter types that bridge the concrete HTTP provider to trait interfaces
//! used by the agent loop and Forge subsystems (reflector/pipeline/learning engine).
//!
//! Shared by both nemesis-web handlers and the nemesisbot binary.
//! Previously duplicated in `agent_factory.rs` and `models.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

// ---------------------------------------------------------------------------
// ProviderAdapter — providers → agent LlmProvider trait
// ---------------------------------------------------------------------------

/// T5（多模态，goal 2026-09-03）：agent LlmMessage → provider Message 的统一
/// 转换。gateway 的 [`ProviderAdapter`] 与 CLI `agent` 命令的 adapter 共用
/// （单一真相源，此前两处手写重复的转换收敛于此）。
///
/// 转换规则：`images` 为空 → 纯文本字符串形态（与历史请求逐字节一致，prompt
/// cache 前缀不破）；`images` 非空 → `MessageContent::Parts` 数组形态——文本
/// part 在前（非空时），每张图片产出 `[图片: <path>]` 标注 part + 图片 part
/// （Base64；`detail: None` 走服务端 auto，goal P2.2 锁定的线上形态）。
pub fn agent_message_to_provider(
    m: nemesis_agent::r#loop::LlmMessage,
) -> nemesis_providers::types::Message {
    use nemesis_providers::types::{ContentPart, ImageSource, MessageContent};

    let content = if m.images.is_empty() {
        MessageContent::Text(m.content)
    } else {
        let mut parts: Vec<ContentPart> = Vec::with_capacity(m.images.len() * 2 + 1);
        if !m.content.is_empty() {
            parts.push(ContentPart::Text { text: m.content });
        }
        for img in m.images {
            parts.push(ContentPart::Text {
                text: format!("[图片: {}]", img.path),
            });
            parts.push(ContentPart::Image {
                image: ImageSource::Base64 {
                    media_type: img.media_type,
                    data: img.data,
                },
                detail: None,
            });
        }
        MessageContent::Parts(parts)
    };

    nemesis_providers::types::Message {
        role: m.role,
        content,
        tool_calls: m
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| nemesis_providers::types::ToolCall {
                id: tc.id,
                call_type: Some("function".to_string()),
                function: Some(nemesis_providers::types::FunctionCall {
                    name: tc.name,
                    arguments: tc.arguments,
                }),
                name: None,
                arguments: None,
            })
            .collect(),
        tool_call_id: m.tool_call_id,
        timestamp: None,
        reasoning_content: m.reasoning_content,
        extra: HashMap::new(),
    }
}

/// Wraps a `nemesis_providers::LLMProvider` so it satisfies the agent's
/// `LlmProvider` trait.
///
/// Converts between provider and agent message/tool types and delegates
/// chat calls to the underlying HTTP provider.
pub struct ProviderAdapter {
    inner: Arc<dyn nemesis_providers::router::LLMProvider>,
    default_model: String,
}

impl ProviderAdapter {
    pub fn new(
        inner: Arc<dyn nemesis_providers::router::LLMProvider>,
        default_model: String,
    ) -> Self {
        Self {
            inner,
            default_model,
        }
    }
}

/// L10（2026-10-13）：FailoverError → loop 层 String 的单一映射源（chat /
/// chat_streamed 共用）。429 Retry-After 保真（2026-09-17 BUG 文档裁决④⑧）
/// ——` (retry_after=Ns)` 后缀供 loop 层子串提取后取 max（上游要求 vs 本地
/// 阶梯），不引入跨 crate 依赖。
fn map_provider_error(e: &nemesis_providers::failover::FailoverError) -> String {
    match e {
        nemesis_providers::failover::FailoverError::RateLimit {
            retry_after: Some(secs),
            ..
        } => format!("{} (retry_after={}s)", e, secs),
        _ => format!("{}", e),
    }
}

/// L10：流式不支持探测——`chat_stream` trait 缺省实现的诚实错误文本
/// （"streaming (chat_stream) not implemented for this provider"）。
/// 命中 = 该 provider 没有流式 lane（CLI 型 / 未实现），chat_streamed 静默
/// 回落非流式调用。
fn is_stream_unsupported(e: &nemesis_providers::failover::FailoverError) -> bool {
    format!("{e}").contains("not implemented")
}

/// L10：agent 层 ChatOptions → provider 层（chat / chat_streamed 共用转换，
/// 单一真相源——此前内联在 chat()）。
fn agent_options_to_provider(
    options: Option<nemesis_agent::types::ChatOptions>,
) -> nemesis_providers::types::ChatOptions {
    match options {
        Some(opts) => nemesis_providers::types::ChatOptions {
            temperature: opts.temperature.map(|t| t as f64),
            max_tokens: opts.max_tokens.map(|t| t as i64),
            top_p: opts.top_p.map(|p| p as f64),
            stop: opts.stop,
            reasoning_effort: opts.reasoning_effort.clone(), // H4: tier passes through
            extra: HashMap::new(),
        },
        None => nemesis_providers::types::ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(8192),
            top_p: None,
            stop: None,
            reasoning_effort: None,
            extra: HashMap::new(),
        },
    }
}

/// L10：agent 层 ToolDefinition → provider 层（同上共用转换）。
fn agent_tools_to_provider(
    tools: Vec<nemesis_agent::types::ToolDefinition>,
) -> Vec<nemesis_providers::types::ToolDefinition> {
    tools
        .into_iter()
        .map(|t| nemesis_providers::types::ToolDefinition {
            tool_type: t.tool_type,
            function: nemesis_providers::types::ToolFunctionDefinition {
                name: t.function.name,
                description: t.function.description,
                parameters: t.function.parameters,
            },
        })
        .collect()
}

/// L10：provider LLMResponse → agent LlmResponse（同上共用映射；finished
/// 判定见函数内注记——工具调用在场恒未终答）。
fn provider_response_to_agent(
    resp: nemesis_providers::types::LLMResponse,
) -> nemesis_agent::r#loop::LlmResponse {
    use nemesis_agent::types::ToolCallInfo as AgentToolCallInfo;

    let tool_calls: Vec<AgentToolCallInfo> = resp
        .tool_calls
        .into_iter()
        .filter_map(|tc| {
            let func = tc.function?;
            Some(AgentToolCallInfo {
                id: tc.id,
                name: func.name,
                arguments: func.arguments,
            })
        })
        .collect();
    // finished 判定：工具调用在场 ⇒ 恒未终答（loop 侧必须 dispatch）。此前
    // `|| finish_reason == "stop"` 会被异常上游（stop 邮票 + tools 在场）静默
    // 丢掉工具调用——judge_final_answer 见 finished=true 直接当终答
    // （2026-10-13 wal_crash 实证；与 chat() 共用同源语义）。
    let finished = tool_calls.is_empty();
    nemesis_agent::r#loop::LlmResponse {
        content: resp.content,
        tool_calls,
        finished,
        reasoning_content: resp.reasoning_content,
        usage: resp
            .usage
            .map(|u| nemesis_agent::loop_executor::ObserverUsageInfo {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                cached_tokens: u.cached_tokens,
                cache_creation_tokens: u.cache_creation_tokens,
                cache_read_tokens: u.cache_read_tokens,
            }),
        raw_request_body: resp.raw_request_body,
        raw_response_body: resp.raw_response_body,
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ProviderAdapter {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> std::result::Result<nemesis_agent::r#loop::LlmResponse, String> {
        let model_to_use = if model.is_empty() {
            &self.default_model
        } else {
            model
        };

        // T5：转换收敛到统一 helper（含多模态 images → Parts 分支）。
        let provider_messages: Vec<nemesis_providers::types::Message> = messages
            .into_iter()
            .map(agent_message_to_provider)
            .collect();
        let provider_options = agent_options_to_provider(options);
        let provider_tools = agent_tools_to_provider(tools);

        match self
            .inner
            .chat(
                &provider_messages,
                &provider_tools,
                model_to_use,
                &provider_options,
            )
            .await
        {
            Ok(resp) => Ok(provider_response_to_agent(resp)),
            Err(e) => {
                warn!("[LlmBridge] LLM provider error: {}", e);
                Err(map_provider_error(&e))
            }
        }
    }

    /// L10：流式 lane 覆写。真实流式经 `inner.chat_stream`（SSE 逐块）→
    /// 增量同步回调 `on_delta` + 聚合成 [`LlmResponse`]（语义与 `chat` 一致：
    /// 同参同返回，finished 判定同源）。三个诚实回落：
    /// - 首块即 "not implemented"（trait 缺省实现）→ 该 provider 无流式
    ///   lane，静默回落 `inner.chat`（非流式），成功后合成一条全量增量；
    /// - 流中途 Err → `map_provider_error`（与 chat 同一映射，429 保真）；
    /// - EOF 无 finish_reason（上游早断）→ 按 tool_calls 推导 finished，
    ///   不虚构终止原因。
    /// raw_request_body / raw_response_body 恒 None（流式路径不缓存原文——
    /// raw 日志模式在此类调用下显示缺省，诚实边界）。
    async fn chat_streamed(
        &self,
        model: &str,
        messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        tools: Vec<nemesis_agent::types::ToolDefinition>,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> std::result::Result<nemesis_agent::r#loop::LlmResponse, String> {
        let model_to_use = if model.is_empty() {
            &self.default_model
        } else {
            model
        };
        let provider_messages: Vec<nemesis_providers::types::Message> = messages
            .into_iter()
            .map(agent_message_to_provider)
            .collect();
        let provider_options = agent_options_to_provider(options);
        let provider_tools = agent_tools_to_provider(tools);

        let mut rx = self.inner.chat_stream(
            &provider_messages,
            &provider_tools,
            model_to_use,
            &provider_options,
        );

        // 显式 HRTB 重绑不再需要：回调取 owned String（见 trait 方法头），
        // async_trait 的生命周期展开不再制造 trait object 统一失败。

        // 首块探测（见方法头回落①）。
        let mut content = String::new();
        let mut last_tool_calls: Vec<nemesis_providers::types::ToolCall> = Vec::new();
        let mut usage: Option<nemesis_providers::types::UsageInfo> = None;
        let mut reasoning_content: Option<String> = None;
        let first = match rx.recv().await {
            None => {
                // 空流（发送端即断）：与"流不支持"同判——回落非流式。
                tracing::debug!(
                    "[LlmBridge] stream closed before first chunk, falling back to non-stream"
                );
                return self
                    .fallback_non_stream(
                        &provider_messages,
                        &provider_tools,
                        model_to_use,
                        &provider_options,
                        on_delta,
                    )
                    .await;
            }
            Some(item) => item,
        };
        if let Err(e) = &first {
            if is_stream_unsupported(e) {
                return self
                    .fallback_non_stream(
                        &provider_messages,
                        &provider_tools,
                        model_to_use,
                        &provider_options,
                        on_delta,
                    )
                    .await;
            }
            warn!("[LlmBridge] LLM stream error: {}", e);
            return Err(map_provider_error(e));
        }
        let mut chunk = first.ok();
        loop {
            if let Some(c) = chunk.take() {
                if !c.delta.is_empty() {
                    on_delta(c.delta.clone());
                    content.push_str(&c.delta);
                }
                if !c.tool_calls.is_empty() {
                    last_tool_calls = c.tool_calls;
                }
                if c.usage.is_some() {
                    usage = c.usage;
                }
                if c.reasoning_content.is_some() {
                    reasoning_content = c.reasoning_content;
                }
            }
            match rx.recv().await {
                Some(Ok(next)) => chunk = Some(next),
                Some(Err(e)) => {
                    warn!("[LlmBridge] LLM stream error: {}", e);
                    return Err(map_provider_error(&e));
                }
                None => break,
            }
        }

        let tool_calls: Vec<nemesis_agent::types::ToolCallInfo> = last_tool_calls
            .into_iter()
            .filter_map(|tc| {
                let func = tc.function?;
                Some(nemesis_agent::types::ToolCallInfo {
                    id: tc.id,
                    name: func.name,
                    arguments: func.arguments,
                })
            })
            .collect();
        // finished 与 provider_response_to_agent 同源：工具调用在场恒未终答
        // （"stop" 邮票 + tools 在场 = 异常上游，不丢工具调用，2026-10-13
        // wal_crash 实证）。
        let finished = tool_calls.is_empty();
        Ok(nemesis_agent::r#loop::LlmResponse {
            content,
            tool_calls,
            finished,
            reasoning_content,
            usage: usage.map(|u| nemesis_agent::loop_executor::ObserverUsageInfo {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                cached_tokens: u.cached_tokens,
                cache_creation_tokens: u.cache_creation_tokens,
                cache_read_tokens: u.cache_read_tokens,
            }),
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

impl ProviderAdapter {
    /// L10：chat_streamed 的非流式回落（provider 无流式 lane / 空流）。
    /// 复用已转换的 provider 侧入参（不重复拷贝 agent 层 messages）；成功后
    /// 合成一条全量增量（loop 侧 StreamingLlmProvider 约定——非流式 provider
    /// 也产生一次完整「流」）。
    async fn fallback_non_stream(
        &self,
        provider_messages: &[nemesis_providers::types::Message],
        provider_tools: &[nemesis_providers::types::ToolDefinition],
        model_to_use: &str,
        provider_options: &nemesis_providers::types::ChatOptions,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> std::result::Result<nemesis_agent::r#loop::LlmResponse, String> {
        match self
            .inner
            .chat(
                provider_messages,
                provider_tools,
                model_to_use,
                provider_options,
            )
            .await
        {
            Ok(resp) => {
                let mapped = provider_response_to_agent(resp);
                if !mapped.content.is_empty() {
                    on_delta(mapped.content.clone());
                }
                Ok(mapped)
            }
            Err(e) => {
                warn!("[LlmBridge] LLM provider error: {}", e);
                Err(map_provider_error(&e))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ForgeProviderBridge — providers → forge LLMCaller trait (async)
// ---------------------------------------------------------------------------

/// Async LLM provider adapter for Forge's Reflector + Pipeline.
///
/// Implements `LLMCaller` trait by delegating to the shared LLM provider.
#[cfg(feature = "forge")]
pub struct ForgeProviderBridge {
    provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

#[cfg(feature = "forge")]
impl ForgeProviderBridge {
    pub fn new(provider: Arc<dyn nemesis_providers::router::LLMProvider>, model: String) -> Self {
        Self { provider, model }
    }
}

#[cfg(feature = "forge")]
#[async_trait::async_trait]
impl nemesis_forge::reflector_llm::LLMCaller for ForgeProviderBridge {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: Option<i64>,
    ) -> std::result::Result<String, String> {
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".to_string(),
                content: system_prompt.to_string().into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".to_string(),
                content: user_prompt.to_string().into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            },
        ];

        let options = nemesis_providers::types::ChatOptions {
            temperature: Some(0.7),
            max_tokens,
            top_p: None,
            stop: None,
            // 显式禁思考（aux 杂务三层治理 2026-09-28）：forge 草稿/评审/
            // 修复/语义反思都是杂务短输出，思考 token 计入 max_tokens 只会
            // 烧穿预算（真机 mt=500 间歇烧穿实证）。anthropic lane 映射为
            // thinking:{type:"disabled"}；openai-compat lane 过滤不透传。
            reasoning_effort: Some("off".to_string()),
            extra: HashMap::new(),
        };

        // 杂务旁路护栏（nemesis_agent::loop::bypass_llm，重试版）：墙钟超时
        // + 空输出校验 + 空输出/瞬态失败同窗口重试一次——Forge 评审/反思/
        // 草稿是千 token 级短输出，挂死连接不拖住后台学习循环；空文本不再
        // 当成功结果透传（下游本就把空串当无效），采样抖动由重试兜底。
        let model = self.model.clone();
        let provider = self.provider.clone();
        nemesis_agent::r#loop::guarded_llm_call_retrying(
            "forge-llm",
            nemesis_agent::r#loop::AUX_FORGE_TIMEOUT,
            move || {
                let messages = messages.clone();
                let options = options.clone();
                let provider = provider.clone();
                let model = model.clone();
                async move {
                    provider
                        .chat(&messages, &[], &model, &options)
                        .await
                        .map_err(|e| format!("{:?}", e))
                        .map(|r| r.content)
                }
            },
        )
        .await
    }
}

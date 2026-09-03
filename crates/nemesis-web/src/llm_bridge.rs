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

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ProviderAdapter {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> std::result::Result<nemesis_agent::r#loop::LlmResponse, String> {
        use nemesis_agent::types::ToolCallInfo as AgentToolCallInfo;

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

        let provider_options = match options {
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
        };

        let provider_tools: Vec<nemesis_providers::types::ToolDefinition> = tools
            .into_iter()
            .map(|t| nemesis_providers::types::ToolDefinition {
                tool_type: t.tool_type,
                function: nemesis_providers::types::ToolFunctionDefinition {
                    name: t.function.name,
                    description: t.function.description,
                    parameters: t.function.parameters,
                },
            })
            .collect();

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
            Ok(resp) => {
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
                let finished = tool_calls.is_empty() || resp.finish_reason == "stop";
                Ok(nemesis_agent::r#loop::LlmResponse {
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
                })
            }
            Err(e) => {
                warn!("[LlmBridge] LLM provider error: {}", e);
                Err(format!("{}", e))
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
            reasoning_effort: None,
            extra: HashMap::new(),
        };

        let response = self
            .provider
            .chat(&messages, &[], &self.model, &options)
            .await
            .map_err(|e| format!("{:?}", e))?;

        if response.content.is_empty() && response.tool_calls.is_empty() {
            Err("LLM returned no content".to_string())
        } else {
            Ok(response.content)
        }
    }
}

//! Protocol types for LLM communication.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A tool call from the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, serde_json::Value>>,
}

/// A function call within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// Reasoning content from thinking-mode models (e.g., DeepSeek R1, GLM).
    /// Must be passed back to the API in subsequent turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// Passthrough for any unknown fields from the API response.
    /// Captured via serde flatten so future API fields are never silently dropped.
    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
    /// Raw HTTP request body sent to the LLM API (for logging).
    #[serde(skip)]
    pub raw_request_body: Option<serde_json::Value>,
    /// Raw HTTP response body received from the LLM API (for logging).
    #[serde(skip)]
    pub raw_response_body: Option<String>,
}

/// Token usage info.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    /// Cached prompt tokens (DeepSeek: prompt_cache_hit_tokens, OpenAI: cached_tokens in prompt_tokens_details).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "prompt_cache_hit_tokens"
    )]
    pub cached_tokens: Option<i64>,
    /// Cache creation tokens (Anthropic: cache_creation_input_tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    /// Cache read tokens (Anthropic: cache_read_input_tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
}

/// 图像细节档（OpenAI vision 计费档位；透传，None = 服务端 auto）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageDetail {
    Low,
    High,
    Auto,
}

/// 图像来源（provider 中立）：URL 交 provider 拉取，Base64 为 bot 侧已读字节。
/// 线上格式由各 provider 适配层转换（OpenAI: image_url.data URI；Anthropic: source.base64/url）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ImageSource {
    Url(String),
    Base64 { media_type: String, data: String },
}

/// 多模态内容部分（D1，已批准；真相源 §2 P1.1）。
/// internally tagged（`type` 字段判别）：Text / Image。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        image: ImageSource,
        /// 视觉计费档；None 不传（D2：成本旋钮，默认服务端 auto）。
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
}

/// 消息内容多态（D1=A，已批准）：纯文本保持字符串形态（字节兼容现网请求
/// 与 prompt cache 前缀），含图时为数组形态。
///
/// 序列化契约（字节快照测试锁定，goal T1/T2 纪律 6）：
/// - `Text`   → JSON 字符串（与历史 `content: String` 逐字节一致）
/// - `Parts`  → JSON 数组（每项为 ContentPart；agent 层保证纯文本折叠为 Text）
///
/// 反序列化接受字符串 / 数组 / null —— 旧集群续行快照 content 恒为字符串，
/// 必须可加载（goal T1 向后兼容红线；null 为兜底宽容）。
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    /// 纯文本（默认；序列化为 JSON 字符串）。
    Text(String),
    /// 多模态部分数组（序列化为 JSON 数组）。
    Parts(Vec<ContentPart>),
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

impl Serialize for MessageContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Parts(parts) => parts.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ContentVisitor;
        impl<'de> serde::de::Visitor<'de> for ContentVisitor {
            type Value = MessageContent;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or an array of content parts")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(MessageContent::Text(v.to_string()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(MessageContent::Text(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut parts = Vec::new();
                while let Some(p) = seq.next_element::<ContentPart>()? {
                    parts.push(p);
                }
                Ok(MessageContent::Parts(parts))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(MessageContent::Text(String::new()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(MessageContent::Text(String::new()))
            }
        }
        deserializer.deserialize_any(ContentVisitor)
    }
}

impl MessageContent {
    /// 纯文本视图：Text 返回 Some；Parts 返回 None（即便只含 text part——
    /// 取文本用 [`to_text`](Self::to_text)，形态判断用
    /// [`is_pure_text`](Self::is_pure_text)）。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s.as_str()),
            MessageContent::Parts(_) => None,
        }
    }

    /// 取文本：Text 原样；Parts 拼接全部 text part（"\n" 分隔，图片 part 跳过）。
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            MessageContent::Text(s) => s.is_empty(),
            MessageContent::Parts(p) => p.is_empty(),
        }
    }

    /// 是否为纯文本形态（agent 层折叠后的常态）。
    pub fn is_pure_text(&self) -> bool {
        matches!(self, MessageContent::Text(_))
    }

    /// 文本视图是否含子串（对齐 String::contains 的迁移便捷方法；
    /// Parts 时只查 text part 拼接结果）。
    pub fn contains(&self, needle: &str) -> bool {
        self.to_text().contains(needle)
    }

    /// CLI 委派类降级投影（D7）：取文本视图；含图时追加"不支持视觉输入"
    /// 占位说明——诚实降级，不静默丢图、不假装传了。
    pub fn to_prompt_text_with_image_note(&self) -> String {
        let n = self.images().len();
        let text = self.to_text();
        if n > 0 {
            format!(
                "{}\n[用户附带 {} 张图片（当前委派通道不支持视觉输入）]",
                text, n
            )
        } else {
            text
        }
    }

    /// 是否含图片部分。
    pub fn has_image(&self) -> bool {
        !self.images().is_empty()
    }

    /// 全部图片部分（含来源与 detail）。
    pub fn images(&self) -> Vec<&ContentPart> {
        match self {
            MessageContent::Text(_) => Vec::new(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter(|p| matches!(p, ContentPart::Image { .. }))
                .collect(),
        }
    }
}

impl std::fmt::Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_text())
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_string())
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl PartialEq<&str> for MessageContent {
    fn eq(&self, other: &&str) -> bool {
        self.as_text() == Some(*other)
    }
}

/// A message in the conversation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<chrono::DateTime<chrono::Local>>,
    /// Reasoning content from thinking-mode models (e.g., DeepSeek R1, GLM).
    /// Must be passed back to the API in subsequent assistant turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// Passthrough for any unknown fields from the API.
    /// Captured via serde flatten so future API fields are never silently dropped.
    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Message {
    /// 纯文本消息构造（其余字段取默认；迁移便捷方法，减少字面量样板）。
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        }
    }

    /// 多模态消息构造（content 为 Parts 形态）。
    pub fn parts(role: impl Into<String>, parts: Vec<ContentPart>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Parts(parts),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        }
    }

    /// 内容文本（Text 原样；Parts 拼 text part）。消费方迁移便捷方法。
    pub fn content_str(&self) -> String {
        self.content.to_text()
    }
}

/// 单条消息的 content → OpenAI 兼容请求 content 值。
/// - `Text`   → JSON 字符串（字节兼容现网请求，prompt cache 前缀不变）
/// - `Parts`  → 数组：Text → `{"type":"text",text}`；
///   Base64 图 → `{"type":"image_url","image_url":{"url":"data:<media_type>;base64,<data>"}}`；
///   Url 图 → 原样 url；`detail` 透传（None 不传）。
pub fn openai_content_value(parts: &[ContentPart]) -> serde_json::Value {
    serde_json::Value::Array(
        parts
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => serde_json::json!({
                    "type": "text",
                    "text": text,
                }),
                ContentPart::Image { image, detail } => {
                    let url = match image {
                        ImageSource::Url(u) => u.clone(),
                        ImageSource::Base64 { media_type, data } => {
                            format!("data:{};base64,{}", media_type, data)
                        }
                    };
                    let mut image_url = serde_json::Map::new();
                    image_url.insert("url".to_string(), serde_json::Value::String(url));
                    if let Some(d) = detail {
                        image_url.insert("detail".to_string(), serde_json::json!(d));
                    }
                    serde_json::json!({
                        "type": "image_url",
                        "image_url": serde_json::Value::Object(image_url),
                    })
                }
            })
            .collect(),
    )
}

/// 消息数组 → OpenAI 兼容请求 `messages` 值（openai_compat / http_provider 共用）：
/// 纯文本消息整体序列化照旧（content 保持字符串）；Parts 消息仅替换 content
/// 字段为数组（其余字段——role/tool_calls/tool_call_id/reasoning_content——序列化不变）。
pub fn messages_to_openai_json(messages: &[Message]) -> serde_json::Value {
    serde_json::Value::Array(
        messages
            .iter()
            .map(|msg| {
                let mut v = serde_json::to_value(msg).expect("Message serialize is infallible");
                if let (MessageContent::Parts(parts), Some(obj)) = (&msg.content, v.as_object_mut())
                {
                    obj.insert("content".to_string(), openai_content_value(parts));
                }
                v
            })
            .collect(),
    )
}

/// Tool definition for LLM API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
    pub function: ToolFunctionDefinition,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// Tool function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Chat completion request options.
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub top_p: Option<f64>,
    pub stop: Option<Vec<String>>,
    pub extra: HashMap<String, serde_json::Value>,
    /// H4 (U16 half): reasoning-effort tier ("low"|"medium"|"high"; None =
    /// send nothing). Providers translate as their wire format requires
    /// (OpenAI-compatible: `reasoning_effort`; Anthropic: `thinking`
    /// budget_tokens via a fixed tier→budget map).
    pub reasoning_effort: Option<String>,
}

/// Model configuration with primary model and fallback list.
///
/// Used by the failover system to determine which models to try
/// when the primary model is unavailable.
///
/// Mirrors the Go `ModelConfig` struct from `module/providers/types.go`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    /// The primary model to use.
    #[serde(default)]
    pub primary: String,
    /// Ordered list of fallback models to try if the primary fails.
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

impl ProviderModelConfig {
    /// Create a new ModelConfig with just a primary model and no fallbacks.
    pub fn new(primary: &str) -> Self {
        Self {
            primary: primary.to_string(),
            fallbacks: Vec::new(),
        }
    }

    /// Create a ModelConfig with a primary model and fallback list.
    pub fn with_fallbacks(primary: &str, fallbacks: &[&str]) -> Self {
        Self {
            primary: primary.to_string(),
            fallbacks: fallbacks.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Get all model names in priority order (primary first, then fallbacks).
    pub fn all_models(&self) -> Vec<&str> {
        let mut models = vec![self.primary.as_str()];
        for fb in &self.fallbacks {
            models.push(fb.as_str());
        }
        models
    }

    /// Returns true if there are any fallback models configured.
    pub fn has_fallbacks(&self) -> bool {
        !self.fallbacks.is_empty()
    }
}

/// Token source type for providers that support OAuth or token refresh.
///
/// This is a simplified version of the Go `createCodexTokenSource` /
/// `createClaudeTokenSource` functions. The actual credential loading
/// is handled by `codex_credentials` module and `auth` module in Go.
/// In Rust, we provide this enum to represent the token source type,
/// and the actual loading is done at construction time or via callbacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenSourceType {
    /// Static API key, no refresh needed.
    Static,
    /// OAuth-based token with refresh capability.
    OAuth,
    /// CLI-based credentials (e.g., from ~/.codex/auth.json).
    CliCredentials,
}

#[cfg(test)]
mod tests;

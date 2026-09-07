//! Channel message types.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Inbound message from a channel to the agent engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub media: Vec<MediaAttachment>,
    pub session_key: String,
    pub correlation_id: String,
    /// Optional metadata for routing (peer_kind, peer_id, account_id, guild_id, team_id, etc.)
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// Whether to inject voice playback prompt in AgentLoop.
    #[serde(default)]
    pub voice_playback: Option<bool>,
}

// ---------------------------------------------------------------------------
// I5（devtool-upgrade 阶段 7）：客户端上报的「当前打开文件」
// ---------------------------------------------------------------------------

/// 打开文件列表条数上限。超出保留前 N 条（协议约定顺序 = 上报顺序，
/// 首条最近活跃），其余诚实丢弃——上报是尽力而为的上下文信号，不是请求
/// 参数，超限不报错。
pub const MAX_OPEN_FILES: usize = 20;

/// 单条路径长度上限（UTF-8 字符数）。超长条目整条丢弃而非截断——截断后的
/// 路径指向不存在的文件，对模型是误导。
pub const MAX_OPEN_FILE_PATH_LEN: usize = 1024;

/// 清洗客户端上报的打开文件路径列表：trim / 去空 / 去重保序 /
/// 条数与单条长度封顶。web 通道写入端与 AgentLoop 渲染端共用此单点，
/// 两端看到的列表永远一致。
pub fn sanitize_open_files(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut cleaned = Vec::new();
    for p in paths {
        let p = p.trim();
        if p.is_empty() || p.chars().count() > MAX_OPEN_FILE_PATH_LEN {
            continue;
        }
        if seen.insert(p.to_string()) {
            cleaned.push(p.to_string());
        }
        if cleaned.len() >= MAX_OPEN_FILES {
            break;
        }
    }
    cleaned
}

/// 从 [`InboundMessage::metadata`] 解析 `open_files` 键（web 通道写入的
/// JSON 字符串数组）。缺失 / 空 / 非法 JSON / 元素非字符串 → 空表（诚实
/// 丢弃，不炸消息）；清洗走 [`sanitize_open_files`] 单点。
pub fn open_files_from_metadata(
    metadata: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    metadata
        .get("open_files")
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .map(sanitize_open_files)
        .unwrap_or_default()
}

/// Extensible per-delivery metadata attached to an `OutboundMessage`.
///
/// Holds attributes that are optional/channel-specific (not every channel
/// cares about every field). Adding a future field here does **not** require
/// touching the ~161 `OutboundMessage` construction sites — they write
/// `meta: Default::default()` and absorb new fields automatically. Channels
/// read what they need and ignore the rest (`#[serde(default)]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundMeta {
    /// Model that produced this assistant response, in `provider/name` form
    /// (e.g. `deepseek/deepseek-v4-flash`). The web channel surfaces it to the
    /// Dashboard as a per-message "供应商·模型名" badge so users can see which
    /// model generated each reply. Other channels ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// L2（devtool-upgrade 阶段 6）：产生本回复的 agent 会话键
    /// （`agent:main:session:{sid}`）。web 通道用它做 chat_event_log 断线
    /// 补拉的环形缓冲键——chat_id 在 web 通道是**连接级** id（重连即变），
    /// 补拉必须按**会话**跨连接寻址。其他通道忽略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

/// Outbound message from the agent engine to a channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    /// Message type: "" = normal, "history" = history response.
    /// Mirrors Go's OutboundMessage.Type field.
    #[serde(default, rename = "type")]
    pub message_type: String,
    /// Extensible delivery metadata (model, future fields). Default = empty.
    #[serde(default)]
    pub meta: OutboundMeta,
}

impl OutboundMessage {
    /// Create a new outbound message with default type.
    pub fn new(channel: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: String::new(),
            meta: OutboundMeta::default(),
        }
    }

    /// Create with a specific message type.
    pub fn with_type(channel: &str, chat_id: &str, content: &str, message_type: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: message_type.to_string(),
            meta: OutboundMeta::default(),
        }
    }
}

/// Media attachment in a message.
///
/// Supports dual serialization for compatibility with Go's `[]string` format:
/// - If only `url` is set (media_type empty, data None), serializes as a plain string.
/// - Otherwise serializes as a full object.
///
/// Deserialization accepts both a plain string (treated as URL) and a full object.
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    pub media_type: String,
    pub url: String,
    pub data: Option<String>,
}

impl Serialize for MediaAttachment {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // If media_type is empty and data is None, serialize as a plain URL string.
        if self.media_type.is_empty() && self.data.is_none() && !self.url.is_empty() {
            serializer.serialize_str(&self.url)
        } else {
            // Full object serialization.
            #[derive(Serialize)]
            struct MediaObj<'a> {
                #[serde(rename = "type", skip_serializing_if = "str::is_empty")]
                media_type: &'a str,
                url: &'a str,
                #[serde(skip_serializing_if = "Option::is_none")]
                data: &'a Option<String>,
            }
            MediaObj {
                media_type: &self.media_type,
                url: &self.url,
                data: &self.data,
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for MediaAttachment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        // Helper struct for object-form deserialization.
        #[derive(Deserialize)]
        struct MediaObj {
            #[serde(rename = "type", default)]
            media_type: String,
            url: String,
            #[serde(default)]
            data: Option<String>,
        }

        // Use a visitor that handles both string and object.
        struct MediaVisitor;

        impl<'de> de::Visitor<'de> for MediaVisitor {
            type Value = MediaAttachment;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a string or a media object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<MediaAttachment, E> {
                Ok(MediaAttachment {
                    media_type: String::new(),
                    url: v.to_string(),
                    data: None,
                })
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<MediaAttachment, A::Error> {
                let obj = MediaObj::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(MediaAttachment {
                    media_type: obj.media_type,
                    url: obj.url,
                    data: obj.data,
                })
            }
        }

        deserializer.deserialize_any(MediaVisitor)
    }
}

/// Channel content types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelContent {
    Text(String),
    Markdown(String),
    Html(String),
}

/// Channel user information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelUser {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub is_admin: bool,
}

#[cfg(test)]
mod tests;

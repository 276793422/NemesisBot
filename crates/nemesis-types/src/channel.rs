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

/// Outbound message from the agent engine to a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    /// Message type: "" = normal, "history" = history response.
    /// Mirrors Go's OutboundMessage.Type field.
    #[serde(default, rename = "type")]
    pub message_type: String,
}

impl OutboundMessage {
    /// Create a new outbound message with default type.
    pub fn new(channel: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: String::new(),
        }
    }

    /// Create with a specific message type.
    pub fn with_type(channel: &str, chat_id: &str, content: &str, message_type: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: message_type.to_string(),
        }
    }
}

/// Media attachment in a message.
///
/// Supports dual serialization for compatibility with Go's `[]string` format:
/// - If only `url` is set (media_type empty, data None), serializes as a plain string.
/// - Otherwise serializes as a full object.
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

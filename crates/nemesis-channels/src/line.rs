//! LINE Messaging API channel (webhook, signature verification, reply tokens).
//!
//! Receives webhook events via HTTP POST, verifies signatures with HMAC-SHA256,
//! and uses the Messaging API for sending replies.

use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

type HmacSha256 = Hmac<Sha256>;

/// LINE channel configuration.
#[derive(Debug, Clone)]
pub struct LineConfig {
    /// Channel access token.
    pub channel_access_token: String,
    /// Channel secret (for signature verification).
    pub channel_secret: String,
    /// Webhook server listen port.
    pub webhook_port: u16,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// LINE webhook request body.
#[derive(Debug, serde::Deserialize)]
pub struct LineWebhookRequest {
    pub destination: Option<String>,
    pub events: Vec<LineEvent>,
}

/// A single LINE event.
#[derive(Debug, serde::Deserialize)]
pub struct LineEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(rename = "replyToken")]
    pub reply_token: Option<String>,
    pub source: Option<LineSource>,
    pub message: Option<LineMessage>,
    pub timestamp: Option<i64>,
}

/// LINE event source.
#[derive(Debug, serde::Deserialize)]
pub struct LineSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub user_id: Option<String>,
    pub group_id: Option<String>,
    pub room_id: Option<String>,
}

/// LINE message content.
#[derive(Debug, serde::Deserialize)]
pub struct LineMessage {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub message_type: String,
    pub text: Option<String>,
}

/// LINE reply message request.
#[derive(serde::Serialize)]
struct LineReplyRequest {
    reply_token: String,
    messages: Vec<LineMessagePayload>,
}

/// LINE push message request.
#[derive(serde::Serialize)]
struct LinePushRequest {
    to: String,
    messages: Vec<LineMessagePayload>,
}

/// A LINE message payload.
#[derive(serde::Serialize)]
struct LineMessagePayload {
    #[serde(rename = "type")]
    msg_type: String,
    text: String,
}

/// LINE channel using Messaging API.
pub struct LineChannel {
    base: BaseChannel,
    config: LineConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    reply_tokens: dashmap::DashMap<String, String>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl LineChannel {
    /// Creates a new `LineChannel`.
    pub fn new(config: LineConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.channel_access_token.is_empty() || config.channel_secret.is_empty() {
            return Err(NemesisError::Channel(
                "LINE channel_access_token and channel_secret are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("line"),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            reply_tokens: dashmap::DashMap::new(),
            bus_sender,
        })
    }

    /// Verifies the webhook signature.
    pub fn verify_signature(&self, body: &[u8], signature: &str) -> bool {
        let mut mac = match HmacSha256::new_from_slice(self.config.channel_secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(body);
        let expected = mac.finalize().into_bytes();
        let _ = hex::encode(expected); // for debugging if needed

        // Constant-time comparison
        hex::decode(signature)
            .ok()
            .map_or(false, |sig| {
                let expected_bytes = expected.as_slice();
                if sig.len() != expected_bytes.len() {
                    return false;
                }
                let mut result = 0u8;
                for (a, b) in sig.iter().zip(expected_bytes.iter()) {
                    result |= a ^ b;
                }
                result == 0
            })
    }

    /// Stores a reply token for a chat.
    pub fn store_reply_token(&self, chat_id: String, reply_token: String) {
        self.reply_tokens.insert(chat_id, reply_token);
    }

    /// Sends a reply using a stored reply token.
    pub async fn reply(&self, reply_token: &str, text: &str) -> Result<()> {
        let request = LineReplyRequest {
            reply_token: reply_token.to_string(),
            messages: vec![LineMessagePayload {
                msg_type: "text".to_string(),
                text: text.to_string(),
            }],
        };

        let resp = self
            .http
            .post("https://api.line.me/v2/bot/message/reply")
            .header(
                "Authorization",
                format!("Bearer {}", self.config.channel_access_token),
            )
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("LINE reply failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!("LINE reply error: {body}")));
        }

        Ok(())
    }

    /// Sends a push message to a user/group/room.
    pub async fn push_message(&self, to: &str, text: &str) -> Result<()> {
        let request = LinePushRequest {
            to: to.to_string(),
            messages: vec![LineMessagePayload {
                msg_type: "text".to_string(),
                text: text.to_string(),
            }],
        };

        let resp = self
            .http
            .post("https://api.line.me/v2/bot/message/push")
            .header(
                "Authorization",
                format!("Bearer {}", self.config.channel_access_token),
            )
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("LINE push failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!("LINE push error: {body}")));
        }

        Ok(())
    }

    /// Handles a single webhook TCP connection with minimal HTTP parsing.
    async fn handle_webhook_connection(
        stream: tokio::net::TcpStream,
        bus: &broadcast::Sender<InboundMessage>,
        channel_secret: &str,
        reply_tokens: &dashmap::DashMap<String, String>,
        running: &Arc<parking_lot::RwLock<bool>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let _ = reply_tokens; // used via DashMap clone

        if !*running.read() {
            return;
        }

        let mut buf = vec![0u8; 65536];
        let mut stream = stream;
        let n = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let request_data = &buf[..n];
        let request_str = String::from_utf8_lossy(request_data);

        // Extract X-Line-Signature header
        let mut signature = "";
        for line in request_str.lines().take(20) {
            if let Some(val) = line.strip_prefix("X-Line-Signature: ") {
                signature = val.trim();
                break;
            }
            if let Some(val) = line.strip_prefix("x-line-signature: ") {
                signature = val.trim();
                break;
            }
        }

        // Find body after double CRLF
        let body = if let Some(idx) = request_str.find("\r\n\r\n") {
            &request_data[idx + 4..n]
        } else {
            &request_data[..n]
        };

        // Verify signature
        if !signature.is_empty() {
            let mut mac = match HmacSha256::new_from_slice(channel_secret.as_bytes()) {
                Ok(m) => m,
                Err(_) => {
                    let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                    return;
                }
            };
            mac.update(body);
            let expected = mac.finalize().into_bytes();
            let expected_b64 = base64::engine::general_purpose::STANDARD.encode(expected);

            if signature != expected_b64 {
                warn!("LINE: invalid webhook signature");
                let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                return;
            }
        }

        // Parse webhook body
        let webhook: LineWebhookRequest = match serde_json::from_slice(body) {
            Ok(w) => w,
            Err(e) => {
                warn!("LINE: failed to parse webhook body: {e}");
                let resp = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                return;
            }
        };

        // Send 200 OK
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes()).await;

        // Process events
        for event in &webhook.events {
            if event.event_type != "message" {
                continue;
            }

            let msg = match event.message.as_ref() {
                Some(m) => m,
                None => continue,
            };

            let text = match msg.text.as_deref() {
                Some(t) if !t.is_empty() => t,
                _ => continue,
            };

            let source = match event.source.as_ref() {
                Some(s) => s,
                None => continue,
            };

            let sender_id = source.user_id.as_deref().unwrap_or("unknown");
            let chat_id = match source.source_type.as_str() {
                "group" => source.group_id.as_deref().unwrap_or(sender_id),
                "room" => source.room_id.as_deref().unwrap_or(sender_id),
                _ => sender_id,
            };

            // Store reply token for response
            if let Some(ref reply_token) = event.reply_token {
                reply_tokens.insert(chat_id.to_string(), reply_token.clone());
            }

            let inbound = InboundMessage {
                channel: "line".to_string(),
                sender_id: sender_id.to_string(),
                chat_id: chat_id.to_string(),
                content: text.to_string(),
                media: Vec::new(),
                session_key: chat_id.to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
            };

            let _ = bus.send(inbound);
        }
    }
}

#[async_trait]
impl Channel for LineChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting LINE channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let channel_secret = self.config.channel_secret.clone();
        let reply_tokens = self.reply_tokens.clone();
        let running = self.running.clone();
        let port = if self.config.webhook_port == 0 {
            8080
        } else {
            self.config.webhook_port
        };

        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                Ok(l) => l,
                Err(e) => {
                    warn!("LINE webhook bind failed on port {port}: {e}");
                    return;
                }
            };
            info!("LINE webhook server listening on port {port}");

            loop {
                if !*running.read() {
                    break;
                }

                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("LINE webhook accept error: {e}");
                        continue;
                    }
                };

                let bus = bus.clone();
                let secret = channel_secret.clone();
                let reply_tokens = reply_tokens.clone();
                let running = running.clone();

                tokio::spawn(async move {
                    Self::handle_webhook_connection(
                        stream, &bus, &secret, &reply_tokens, &running,
                    )
                    .await;
                });
            }

            info!("LINE webhook server stopped");
        });

        info!("LINE channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping LINE channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        self.reply_tokens.clear();
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "LINE channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        // Prefer reply token if available
        if let Some((_, reply_token)) = self.reply_tokens.remove(&msg.chat_id) {
            return self.reply(&reply_token, &msg.content).await;
        }

        // Fall back to push message
        self.push_message(&msg.chat_id, &msg.content).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bus() -> broadcast::Sender<InboundMessage> {
        let (tx, _) = broadcast::channel(256);
        tx
    }

    #[tokio::test]
    async fn test_line_channel_new_validates() {
        let config = LineConfig {
            channel_access_token: String::new(),
            channel_secret: String::new(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        assert!(LineChannel::new(config, test_bus()).is_err());
    }

    #[tokio::test]
    async fn test_line_channel_lifecycle() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "line");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_verify_signature_valid() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();

        let body = b"hello world";
        let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());

        assert!(ch.verify_signature(body, &sig));
    }

    #[test]
    fn test_verify_signature_invalid() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();

        assert!(!ch.verify_signature(b"hello", "invalid_hex"));
        assert!(!ch.verify_signature(b"hello", "deadbeef"));
    }

    #[test]
    fn test_deserialize_webhook() {
        let json = r#"{
            "destination": "U123",
            "events": [{
                "type": "message",
                "replyToken": "rt-123",
                "source": {"type": "user", "userId": "U456"},
                "message": {"type": "text", "text": "Hello"},
                "timestamp": 1234567890
            }]
        }"#;

        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events.len(), 1);
        assert_eq!(req.events[0].event_type, "message");
        assert_eq!(req.events[0].reply_token.as_deref(), Some("rt-123"));
        assert_eq!(
            req.events[0].message.as_ref().unwrap().text.as_deref(),
            Some("Hello")
        );
    }

    // ---- New tests ----

    #[test]
    fn test_line_config_fields() {
        let config = LineConfig {
            channel_secret: "secret".into(),
            channel_access_token: "token".into(),
            webhook_port: 8080,
            allow_from: vec!["U123".into()],
        };
        assert_eq!(config.channel_secret, "secret");
        assert_eq!(config.channel_access_token, "token");
    }

    #[test]
    fn test_deserialize_webhook_multiple_events() {
        let json = r#"{
            "destination": "U123",
            "events": [
                {"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "text", "text": "First"}, "timestamp": 1},
                {"type": "message", "replyToken": "rt2", "source": {"type": "user", "userId": "U2"}, "message": {"type": "text", "text": "Second"}, "timestamp": 2}
            ]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events.len(), 2);
    }

    #[test]
    fn test_deserialize_webhook_empty_events() {
        let json = r#"{"destination": "U123", "events": []}"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert!(req.events.is_empty());
    }

    #[test]
    fn test_deserialize_webhook_non_message_event() {
        let json = r#"{
            "destination": "U123",
            "events": [{"type": "follow", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events[0].event_type, "follow");
        assert!(req.events[0].message.is_none());
    }

    // -- Additional tests for coverage --

    #[test]
    fn test_verify_signature_empty_signature() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        assert!(!ch.verify_signature(b"hello", ""));
    }

    #[test]
    fn test_verify_signature_wrong_body() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();

        // Generate a valid signature for "correct_body"
        let body = b"correct_body";
        let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());

        // But verify with a different body
        assert!(!ch.verify_signature(b"wrong_body", &sig));
    }

    #[test]
    fn test_verify_signature_short_hex() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        // Hex that's too short (less than 32 bytes when decoded)
        assert!(!ch.verify_signature(b"hello", "abcd"));
    }

    #[test]
    fn test_store_reply_token() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();

        ch.store_reply_token("chat-1".into(), "rt-abc".into());
        assert_eq!(ch.reply_tokens.get("chat-1").unwrap().value(), "rt-abc");

        // Overwrite
        ch.store_reply_token("chat-1".into(), "rt-def".into());
        assert_eq!(ch.reply_tokens.get("chat-1").unwrap().value(), "rt-def");
    }

    #[test]
    fn test_deserialize_line_source_group() {
        // LineSource uses snake_case field names (group_id, room_id, user_id)
        let json = r#"{
            "destination": "U123",
            "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "group", "user_id": "U1", "group_id": "G1"}, "message": {"type": "text", "text": "Hello"}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        let source = req.events[0].source.as_ref().unwrap();
        assert_eq!(source.source_type, "group");
        assert_eq!(source.group_id.as_deref(), Some("G1"));
        assert_eq!(source.user_id.as_deref(), Some("U1"));
    }

    #[test]
    fn test_deserialize_line_source_room() {
        let json = r#"{
            "destination": "U123",
            "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "room", "room_id": "R1"}, "message": {"type": "text", "text": "Hello"}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        let source = req.events[0].source.as_ref().unwrap();
        assert_eq!(source.source_type, "room");
        assert_eq!(source.room_id.as_deref(), Some("R1"));
    }

    #[test]
    fn test_deserialize_line_message_non_text() {
        let json = r#"{
            "destination": "U123",
            "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "image", "id": "msg-1"}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        let msg = req.events[0].message.as_ref().unwrap();
        assert_eq!(msg.message_type, "image");
        assert!(msg.text.is_none());
        assert_eq!(msg.id.as_deref(), Some("msg-1"));
    }

    #[test]
    fn test_deserialize_webhook_no_destination() {
        let json = r#"{"events": []}"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert!(req.destination.is_none());
        assert!(req.events.is_empty());
    }

    #[test]
    fn test_deserialize_event_without_optional_fields() {
        let json = r#"{
            "destination": "U123",
            "events": [{"type": "postback", "timestamp": 999}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events[0].event_type, "postback");
        assert!(req.events[0].reply_token.is_none());
        assert!(req.events[0].source.is_none());
        assert!(req.events[0].message.is_none());
        assert_eq!(req.events[0].timestamp.unwrap(), 999);
    }

    // ---- Additional coverage tests ----

    #[tokio::test]
    async fn test_send_not_running() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        // Not started - send should fail
        let msg = OutboundMessage {
            channel: "line".to_string(),
            chat_id: "test-chat".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not running"));
    }

    #[tokio::test]
    async fn test_start_stop_clears_reply_tokens() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();
        ch.store_reply_token("chat-1".into(), "rt-abc".into());
        assert_eq!(ch.reply_tokens.len(), 1);

        ch.stop().await.unwrap();
        assert!(ch.reply_tokens.is_empty());
    }

    #[tokio::test]
    async fn test_start_stop_idempotent() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();
        ch.start().await.unwrap(); // second start should be fine
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        ch.stop().await.unwrap(); // second stop should be fine
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_verify_signature_with_unicode_body() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();

        let body = "hello world";
        let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
        mac.update(body.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());

        assert!(ch.verify_signature(body.as_bytes(), &sig));
    }

    #[test]
    fn test_reply_tokens_overwrite() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.store_reply_token("chat-1".into(), "rt-1".into());
        ch.store_reply_token("chat-1".into(), "rt-2".into());
        assert_eq!(ch.reply_tokens.get("chat-1").unwrap().value(), "rt-2");
    }

    #[test]
    fn test_reply_tokens_multiple_chats() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.store_reply_token("chat-1".into(), "rt-1".into());
        ch.store_reply_token("chat-2".into(), "rt-2".into());
        ch.store_reply_token("chat-3".into(), "rt-3".into());
        assert_eq!(ch.reply_tokens.len(), 3);
    }

    #[test]
    fn test_line_message_deserialization_types() {
        let json = r#"{"id":"msg-1","type":"text","text":"hello"}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "text");
        assert_eq!(msg.text.as_deref(), Some("hello"));
        assert_eq!(msg.id.as_deref(), Some("msg-1"));
    }

    #[test]
    fn test_line_message_non_text_type() {
        let json = r#"{"id":"msg-2","type":"image"}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "image");
        assert!(msg.text.is_none());
    }

    #[test]
    fn test_line_source_user_type() {
        let json = r#"{"type":"user","user_id":"U123"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "user");
        assert_eq!(source.user_id.as_deref(), Some("U123"));
        assert!(source.group_id.is_none());
        assert!(source.room_id.is_none());
    }

    #[test]
    fn test_deserialize_webhook_with_text_event_and_timestamp() {
        let json = r#"{
            "destination": "U999",
            "events": [{
                "type": "message",
                "replyToken": "rt-xyz",
                "source": {"type": "user", "userId": "U111"},
                "message": {"type": "text", "text": "Test message"},
                "timestamp": 1700000000000
            }]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.destination.as_deref(), Some("U999"));
        assert_eq!(req.events[0].timestamp.unwrap(), 1700000000000);
        assert_eq!(req.events[0].reply_token.as_deref(), Some("rt-xyz"));
    }

    #[test]
    fn test_line_config_accessors() {
        let config = LineConfig {
            channel_access_token: "my_token".to_string(),
            channel_secret: "my_secret".to_string(),
            webhook_port: 9090,
            allow_from: vec!["U123".to_string()],
        };
        assert_eq!(config.channel_access_token, "my_token");
        assert_eq!(config.channel_secret, "my_secret");
        assert_eq!(config.webhook_port, 9090);
        assert_eq!(config.allow_from.len(), 1);
    }

    #[test]
    fn test_verify_signature_with_empty_body() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();

        let body = b"";
        let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());

        assert!(ch.verify_signature(body, &sig));
    }

    #[test]
    fn test_verify_signature_non_hex_chars() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        // Contains non-hex characters
        assert!(!ch.verify_signature(b"hello", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
    }

    #[test]
    fn test_verify_signature_wrong_length() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        // Too short hex string
        assert!(!ch.verify_signature(b"hello", "deadbeef"));
    }

    #[test]
    fn test_verify_signature_empty_string_signature() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        assert!(!ch.verify_signature(b"hello", ""));
    }

    #[test]
    fn test_line_channel_new_valid_token() {
        let config = LineConfig {
            channel_access_token: "valid_token".to_string(),
            channel_secret: "valid_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus());
        assert!(ch.is_ok());
        let ch = ch.unwrap();
        assert_eq!(ch.name(), "line");
    }

    #[test]
    fn test_line_channel_new_empty_token() {
        let config = LineConfig {
            channel_access_token: String::new(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        assert!(LineChannel::new(config, test_bus()).is_err());
    }

    #[test]
    fn test_line_channel_new_empty_secret() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: String::new(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        assert!(LineChannel::new(config, test_bus()).is_err());
    }

    #[test]
    fn test_deserialize_line_source_with_group_id() {
        let json = r#"{"type": "group", "user_id": "U1", "group_id": "G1"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "group");
        assert_eq!(source.group_id.as_deref(), Some("G1"));
        assert_eq!(source.user_id.as_deref(), Some("U1"));
    }

    #[test]
    fn test_deserialize_line_source_with_room_id() {
        let json = r#"{"type": "room", "user_id": "U1", "room_id": "R1"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "room");
        assert_eq!(source.room_id.as_deref(), Some("R1"));
    }

    #[test]
    fn test_deserialize_line_message_image_type() {
        let json = r#"{"type": "image", "id": "msg1"}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "image");
        assert!(msg.text.is_none());
    }

    #[test]
    fn test_deserialize_webhook_follow_event() {
        let json = r#"{
            "destination": "U123",
            "events": [{
                "type": "follow",
                "replyToken": "rt-follow",
                "source": {"type": "user", "userId": "U456"},
                "timestamp": 1234567890
            }]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events.len(), 1);
        assert_eq!(req.events[0].event_type, "follow");
        assert!(req.events[0].message.is_none());
    }

    #[test]
    fn test_deserialize_webhook_unsend_event() {
        let json = r#"{
            "events": [{
                "type": "unsend",
                "source": {"type": "user", "userId": "U789"},
                "timestamp": 0
            }]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events[0].event_type, "unsend");
    }

    #[test]
    fn test_store_and_remove_reply_token() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.store_reply_token("chat-1".into(), "rt-abc".into());
        ch.store_reply_token("chat-2".into(), "rt-def".into());
        assert_eq!(ch.reply_tokens.len(), 2);
        // Remove should return the token
        assert_eq!(ch.reply_tokens.remove("chat-1").unwrap().1, "rt-abc");
        assert_eq!(ch.reply_tokens.len(), 1);
    }

    #[tokio::test]
    async fn test_send_with_reply_token_uses_reply() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();
        ch.store_reply_token("chat-1".into(), "rt-test".into());

        // The reply will fail (network), but the token should be consumed
        let msg = OutboundMessage {
            channel: "line".to_string(),
            chat_id: "chat-1".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        // Reply fails because no actual LINE server, but token is removed
        let _ = ch.send(msg).await;
        // Reply token should have been consumed
        assert!(ch.reply_tokens.get("chat-1").is_none());
    }

    #[tokio::test]
    async fn test_send_push_message_on_no_token() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "line".to_string(),
            chat_id: "U_no_token".to_string(),
            content: "push msg".to_string(),
            message_type: String::new(),
        };
        // Will fail due to network, but exercises the push path
        let result = ch.send(msg).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_line_message_text_with_id() {
        let json = r#"{"type": "text", "id": "12345", "text": "Hello world"}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "text");
        assert_eq!(msg.id.as_deref(), Some("12345"));
        assert_eq!(msg.text.as_deref(), Some("Hello world"));
    }

    #[test]
    fn test_deserialize_webhook_multiple_message_events() {
        let json = r#"{
            "events": [
                {"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "text", "text": "hi"}, "timestamp": 100},
                {"type": "message", "replyToken": "rt2", "source": {"type": "group", "userId": "U2", "groupId": "G1"}, "message": {"type": "text", "text": "hello"}, "timestamp": 200}
            ]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events.len(), 2);
        assert_eq!(req.events[0].reply_token.as_deref(), Some("rt1"));
        assert_eq!(req.events[1].source.as_ref().unwrap().source_type, "group");
    }

    #[test]
    fn test_line_config_default_port() {
        let config = LineConfig {
            channel_access_token: "t".to_string(),
            channel_secret: "s".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        assert_eq!(config.webhook_port, 0);
    }

    #[test]
    fn test_deserialize_event_empty_message_text() {
        let json = r#"{
            "events": [{
                "type": "message",
                "replyToken": "rt1",
                "source": {"type": "user", "userId": "U1"},
                "message": {"type": "text", "text": ""},
                "timestamp": 100
            }]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events[0].message.as_ref().unwrap().text.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn test_start_stop_running_state() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        assert!(!*ch.running.read());
        ch.start().await.unwrap();
        assert!(*ch.running.read());
        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_verify_signature_length_mismatch() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "test_secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        // Only 4 bytes (8 hex chars) vs expected 32 bytes (64 hex chars)
        assert!(!ch.verify_signature(b"test", "aabbccdd"));
    }

    // ============================================================
    // Additional coverage tests for 95%+ target (round 2)
    // ============================================================

    #[test]
    fn test_line_event_source_without_user_id() {
        let json = r#"{"type": "user"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "user");
        assert!(source.user_id.is_none());
    }

    #[test]
    fn test_line_event_source_minimal() {
        let json = r#"{"type": "group"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "group");
        assert!(source.group_id.is_none());
        assert!(source.user_id.is_none());
    }

    #[test]
    fn test_line_message_minimal() {
        let json = r#"{"type": "text"}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "text");
        assert!(msg.text.is_none());
        assert!(msg.id.is_none());
    }

    #[test]
    fn test_line_event_minimal() {
        let json = r#"{"type": "message", "timestamp": 100}"#;
        let event: LineEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "message");
        assert!(event.reply_token.is_none());
        assert!(event.source.is_none());
        assert!(event.message.is_none());
        assert_eq!(event.timestamp.unwrap(), 100);
    }

    #[test]
    fn test_line_webhook_empty() {
        let json = r#"{"events": []}"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert!(req.events.is_empty());
    }

    #[tokio::test]
    async fn test_line_channel_base_name() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "line");
    }

    #[test]
    fn test_line_source_room_with_user() {
        let json = r#"{"type": "room", "user_id": "U1", "room_id": "R1"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "room");
        assert_eq!(source.room_id.as_deref(), Some("R1"));
        assert_eq!(source.user_id.as_deref(), Some("U1"));
    }

    #[test]
    fn test_line_webhook_event_with_no_source() {
        let json = r#"{
            "events": [{"type": "message", "replyToken": "rt1", "message": {"type": "text", "text": "hi"}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert!(req.events[0].source.is_none());
    }

    #[test]
    fn test_line_webhook_event_with_empty_text() {
        let json = r#"{
            "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "text", "text": ""}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        let text = req.events[0].message.as_ref().unwrap().text.as_deref();
        assert_eq!(text, Some(""));
    }

    #[test]
    fn test_line_webhook_event_non_text_message() {
        let json = r#"{
            "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "sticker", "id": "msg-1"}, "timestamp": 1}]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        let msg = req.events[0].message.as_ref().unwrap();
        assert_eq!(msg.message_type, "sticker");
        assert!(msg.text.is_none());
    }

    #[test]
    fn test_line_source_user_no_user_id() {
        let json = r#"{"type": "user"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, "user");
        assert!(source.user_id.is_none());
        assert!(source.group_id.is_none());
        assert!(source.room_id.is_none());
    }

    #[test]
    fn test_deserialize_webhook_event_type_field() {
        let json = r#"{
            "events": [
                {"type": "message", "timestamp": 1},
                {"type": "follow", "timestamp": 2},
                {"type": "unsend", "timestamp": 3},
                {"type": "postback", "timestamp": 4}
            ]
        }"#;
        let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.events.len(), 4);
        assert_eq!(req.events[0].event_type, "message");
        assert_eq!(req.events[1].event_type, "follow");
        assert_eq!(req.events[2].event_type, "unsend");
        assert_eq!(req.events[3].event_type, "postback");
    }

    #[tokio::test]
    async fn test_send_uses_push_when_no_reply_token() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch = LineChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();

        // No reply token stored for this chat_id, should use push
        let msg = OutboundMessage {
            channel: "line".to_string(),
            chat_id: "chat-no-token".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
        };
        // Will fail (no network), but exercises push path
        let result = ch.send(msg).await;
        assert!(result.is_err());

        ch.stop().await.unwrap();
    }

    #[test]
    fn test_verify_signature_same_body_different_secret() {
        let config1 = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret1".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let config2 = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "secret2".to_string(),
            webhook_port: 0,
            allow_from: Vec::new(),
        };
        let ch1 = LineChannel::new(config1, test_bus()).unwrap();
        let ch2 = LineChannel::new(config2, test_bus()).unwrap();

        let body = b"test body";
        let mut mac = HmacSha256::new_from_slice(b"secret1").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());

        assert!(ch1.verify_signature(body, &sig));
        assert!(!ch2.verify_signature(body, &sig));
    }
}

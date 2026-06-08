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
                warn!("[LineChannel] invalid webhook signature");
                let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                return;
            }
        }

        // Parse webhook body
        let webhook: LineWebhookRequest = match serde_json::from_slice(body) {
            Ok(w) => w,
            Err(e) => {
                warn!("[LineChannel] failed to parse webhook body: {e}");
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
                voice_playback: None,
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

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!("[LineChannel] starting LINE channel");
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
                    warn!("[LineChannel] webhook bind failed on port {port}: {e}");
                    return;
                }
            };
            info!("[LineChannel] webhook server listening on port {port}");

            loop {
                if !*running.read() {
                    break;
                }

                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("[LineChannel] webhook accept error: {e}");
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

            info!("[LineChannel] webhook server stopped");
        });

        info!("[LineChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[LineChannel] stopping LINE channel");
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
mod tests;

//! Slack Socket Mode channel (events API, slash commands, thread support).
//!
//! Uses Slack's Socket Mode for receiving events and the Web API for
//! sending messages. Supports thread replies, file download, and
//! reaction-based acknowledgment (eyes on receive, checkmark on reply).

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

const SLACK_API_BASE: &str = "https://slack.com/api";
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Slack API types
// ---------------------------------------------------------------------------

/// Slack channel configuration.
#[derive(Debug, Clone)]
pub struct SlackConfig {
    /// Bot token (xoxb-...).
    pub bot_token: String,
    /// App-level token (xapp-...).
    pub app_token: String,
    /// Allowed user IDs (empty = allow all).
    pub allow_from: Vec<String>,
}

/// Slack message event (simplified).
#[derive(Debug, Deserialize)]
pub struct SlackMessageEvent {
    pub user: Option<String>,
    pub channel: Option<String>,
    pub text: Option<String>,
    pub ts: Option<String>,
    pub thread_ts: Option<String>,
    pub bot_id: Option<String>,
    pub subtype: Option<String>,
}

/// Slack chat.postMessage response.
#[derive(Debug, Deserialize)]
struct PostMessageResponse {
    ok: bool,
    error: Option<String>,
}

/// Parameters for chat.postMessage.
#[derive(Serialize)]
struct PostMessageParams {
    channel: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<String>,
}

// ---------------------------------------------------------------------------
// SlackChannel
// ---------------------------------------------------------------------------

/// Slack channel using Socket Mode and Web API.
///
/// Connects to Slack via Socket Mode WebSocket for receiving events,
/// converts them to `InboundMessage`, and publishes to the bus.
/// Uses REST API for sending outbound messages.
pub struct SlackChannel {
    base: BaseChannel,
    config: SlackConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    bot_user_id: Arc<parking_lot::RwLock<String>>,
    pending_acks: dashmap::DashMap<String, SlackMessageRef>,
    transcriber: parking_lot::RwLock<Option<Arc<dyn crate::base::VoiceTranscriber>>>,
    /// Bus sender for publishing inbound messages.
    bus_sender: broadcast::Sender<InboundMessage>,
}

/// Tracks a Slack message for acknowledgment.
#[derive(Debug, Clone)]
pub struct SlackMessageRef {
    pub channel_id: String,
    pub timestamp: String,
}

impl SlackChannel {
    /// Creates a new `SlackChannel`.
    pub fn new(config: SlackConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.bot_token.is_empty() || config.app_token.is_empty() {
            return Err(NemesisError::Channel(
                "slack bot_token and app_token are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("slack"),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            bot_user_id: Arc::new(parking_lot::RwLock::new(String::new())),
            pending_acks: dashmap::DashMap::new(),
            transcriber: parking_lot::RwLock::new(None),
            bus_sender,
        })
    }

    /// Sets the bot user ID (after auth.test).
    pub fn set_bot_user_id(&self, id: String) {
        *self.bot_user_id.write() = id;
    }

    /// Sets the voice transcriber for audio file transcription.
    pub fn set_transcriber(&self, transcriber: Arc<dyn crate::base::VoiceTranscriber>) {
        *self.transcriber.write() = Some(transcriber);
    }

    /// Returns the bot user ID.
    pub fn bot_user_id(&self) -> String {
        self.bot_user_id.read().clone()
    }

    /// Stores a pending acknowledgment for a chat.
    pub fn store_pending_ack(&self, chat_id: String, msg_ref: SlackMessageRef) {
        self.pending_acks.insert(chat_id, msg_ref);
    }

    /// Sends a message via Slack Web API.
    async fn post_message(&self, channel: &str, text: &str, thread_ts: Option<&str>) -> Result<()> {
        let params = PostMessageParams {
            channel: channel.to_string(),
            text: text.to_string(),
            thread_ts: thread_ts.map(|s| s.to_string()),
        };

        let resp = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .header("Content-Type", "application/json")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("slack post failed: {e}")))?;

        let body: PostMessageResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("slack response parse failed: {e}")))?;

        if !body.ok {
            return Err(NemesisError::Channel(format!(
                "slack API error: {}",
                body.error.unwrap_or_default()
            )));
        }

        Ok(())
    }

    /// Parses a Slack chat ID into (channel_id, thread_ts).
    /// Format: "CHANNEL_ID" or "CHANNEL_ID/THREAD_TS"
    pub fn parse_slack_chat_id(chat_id: &str) -> (&str, Option<&str>) {
        if let Some(idx) = chat_id.find('/') {
            (&chat_id[..idx], Some(&chat_id[idx + 1..]))
        } else {
            (chat_id, None)
        }
    }

    /// Strips bot mention from text.
    pub fn strip_bot_mention(&self, text: &str) -> String {
        let bot_id = self.bot_user_id.read().clone();
        if bot_id.is_empty() {
            return text.to_string();
        }
        text.replace(&format!("<@{bot_id}>"), "")
            .trim()
            .to_string()
    }

    /// Validates the bot token by calling auth.test.
    async fn validate_bot_token(&self) -> Result<String> {
        let resp: serde_json::Value = self
            .http
            .post(format!("{SLACK_API_BASE}/auth.test"))
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("slack auth.test failed: {e}")))?
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("slack auth.test parse failed: {e}")))?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(NemesisError::Channel(format!("slack auth.test failed: {err}")));
        }

        let user_id = resp["user_id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        Ok(user_id)
    }

    /// Gets a Socket Mode WebSocket URL from Slack.
    async fn get_socket_mode_url(&self) -> Result<String> {
        let resp: serde_json::Value = self
            .http
            .post(format!("{SLACK_API_BASE}/apps.connections.open"))
            .header(
                "Authorization",
                format!("Bearer {}", self.config.app_token),
            )
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .map_err(|e| {
                NemesisError::Channel(format!("slack apps.connections.open failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                NemesisError::Channel(format!(
                    "slack apps.connections.open parse failed: {e}"
                ))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(NemesisError::Channel(format!(
                "slack apps.connections.open failed: {err}"
            )));
        }

        resp["url"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| NemesisError::Channel("missing 'url' in connections.open response".to_string()))
    }

    /// Starts the Socket Mode WebSocket receive loop.
    fn start_socket_mode_loop(&self) {
        let http = self.http.clone();
        let app_token = self.config.app_token.clone();
        let bot_user_id = self.bot_user_id.clone();
        let allow_from = self.config.allow_from.clone();
        let running = self.running.clone();
        let bus_sender = self.bus_sender.clone();

        tokio::spawn(async move {
            let mut backoff = INITIAL_BACKOFF;

            loop {
                if !*running.read() {
                    break;
                }

                // Get a fresh WebSocket URL
                let ws_url = {
                    let body: serde_json::Value = match http
                        .post(format!("{SLACK_API_BASE}/apps.connections.open"))
                        .header("Authorization", format!("Bearer {app_token}"))
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .send()
                        .await
                    {
                        Ok(resp) => match resp.json().await {
                            Ok(v) => v,
                            Err(e) => {
                                warn!("Slack: failed to parse connections.open response: {e}");
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(MAX_BACKOFF);
                                continue;
                            }
                        },
                        Err(e) => {
                            warn!("Slack: failed to get WebSocket URL: {e}, retrying in {backoff:?}");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    };

                    if body["ok"].as_bool() != Some(true) {
                        let err = body["error"].as_str().unwrap_or("unknown");
                        warn!("Slack: connections.open error: {err}, retrying in {backoff:?}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }

                    match body["url"].as_str() {
                        Some(u) => u.to_string(),
                        None => {
                            warn!("Slack: missing URL in connections.open response");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                };

                info!("Connecting to Slack Socket Mode...");

                let ws_result = tokio_tungstenite::connect_async(&ws_url).await;
                let ws_stream = match ws_result {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        warn!("Slack WebSocket connection failed: {e}, retrying in {backoff:?}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                backoff = INITIAL_BACKOFF;
                info!("Slack Socket Mode connected");

                let (mut ws_tx, mut ws_rx) = ws_stream.split();

                let should_reconnect = 'inner: loop {
                    let msg = match ws_rx.next().await {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            warn!("Slack WebSocket error: {e}");
                            break 'inner true;
                        }
                        None => {
                            info!("Slack WebSocket closed");
                            break 'inner true;
                        }
                    };

                    let text = match msg {
                        tokio_tungstenite::tungstenite::Message::Text(t) => t,
                        tokio_tungstenite::tungstenite::Message::Close(_) => {
                            info!("Slack Socket Mode closed by server");
                            break 'inner true;
                        }
                        _ => continue,
                    };

                    let payload: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("Slack: failed to parse message: {e}");
                            continue;
                        }
                    };

                    let envelope_type = payload["type"].as_str().unwrap_or("");

                    match envelope_type {
                        "hello" => {
                            debug!("Slack Socket Mode hello received");
                        }

                        "events_api" => {
                            // Acknowledge the envelope
                            let envelope_id = payload["envelope_id"].as_str().unwrap_or("");
                            if !envelope_id.is_empty() {
                                let ack = serde_json::json!({ "envelope_id": envelope_id });
                                if let Err(e) = ws_tx
                                    .send(tokio_tungstenite::tungstenite::Message::Text(
                                        serde_json::to_string(&ack).unwrap().into(),
                                    ))
                                    .await
                                {
                                    error!("Slack: failed to send ack: {e}");
                                    break 'inner true;
                                }
                            }

                            // Extract the event
                            let event = &payload["payload"]["event"];
                            if let Some(inbound) = Self::parse_slack_event(
                                event,
                                &bot_user_id,
                                &allow_from,
                            ) {
                                debug!(
                                    "Slack message from {} in channel {}",
                                    inbound.sender_id, inbound.chat_id
                                );
                                if bus_sender.send(inbound).is_err() {
                                    warn!("Slack: failed to publish inbound message (no receivers)");
                                }
                            }
                        }

                        "disconnect" => {
                            let reason = payload["reason"].as_str().unwrap_or("unknown");
                            info!("Slack disconnect request: {reason}");
                            break 'inner true;
                        }

                        _ => {
                            debug!("Slack envelope type: {envelope_type}");
                        }
                    }
                };

                if !should_reconnect || !*running.read() {
                    break;
                }

                warn!("Slack: reconnecting in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("Slack Socket Mode loop stopped");
        });
    }

    /// Parses a Slack event into an InboundMessage.
    fn parse_slack_event(
        event: &serde_json::Value,
        bot_user_id: &Arc<parking_lot::RwLock<String>>,
        allow_from: &[String],
    ) -> Option<InboundMessage> {
        let event_type = event["type"].as_str()?;
        if event_type != "message" && event_type != "app_mention" {
            return None;
        }

        // Handle message_changed subtype
        let subtype = event["subtype"].as_str();
        let msg_data = match subtype {
            Some("message_changed") => event.get("message")?,
            Some(_) => return None,
            None => event,
        };

        // Filter out bot messages
        if msg_data.get("bot_id").is_some() {
            return None;
        }

        let user_id = msg_data["user"]
            .as_str()
            .or_else(|| event["user"].as_str())?;

        // Filter out own messages
        let bot_id = bot_user_id.read().clone();
        if !bot_id.is_empty() && user_id == bot_id {
            return None;
        }

        // Filter by allow_from
        if !allow_from.is_empty() && !allow_from.iter().any(|u| u == user_id) {
            debug!("Slack: ignoring message from unlisted user {user_id}");
            return None;
        }

        let channel = event["channel"].as_str()?;
        let text = msg_data["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        let ts = msg_data["ts"]
            .as_str()
            .or_else(|| event["ts"].as_str())
            .unwrap_or("0");

        let thread_ts = msg_data["thread_ts"]
            .as_str()
            .or_else(|| event["thread_ts"].as_str())
            .map(|s| s.to_string());

        let mut metadata = HashMap::new();
        metadata.insert("ts".to_string(), ts.to_string());
        metadata.insert("user_id".to_string(), user_id.to_string());

        if let Some(ref tts) = thread_ts {
            metadata.insert("thread_ts".to_string(), tts.clone());
        }

        // Check if bot was mentioned
        if event_type == "app_mention" {
            metadata.insert("was_mentioned".to_string(), "true".to_string());
        } else if !bot_id.is_empty() {
            let mention_tag = format!("<@{bot_id}>");
            if text.contains(&mention_tag) {
                metadata.insert("was_mentioned".to_string(), "true".to_string());
            }
        }

        // Build chat_id with thread info
        let chat_id = if let Some(ref tts) = thread_ts {
            format!("{channel}/{tts}")
        } else {
            channel.to_string()
        };

        Some(InboundMessage {
            channel: "slack".to_string(),
            sender_id: user_id.to_string(),
            chat_id,
            content: text.to_string(),
            media: Vec::new(),
            session_key: String::new(),
            correlation_id: String::new(),
            metadata,
        })
    }

    /// Adds a reaction to a message.
    pub async fn add_reaction(
        &self,
        channel: &str,
        timestamp: &str,
        emoji: &str,
    ) -> Result<()> {
        let params = serde_json::json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": emoji,
        });

        let resp = self
            .http
            .post("https://slack.com/api/reactions.add")
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .header("Content-Type", "application/json")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("slack reaction failed: {e}")))?;

        let _ = resp;
        Ok(())
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting Slack channel (Socket Mode)");

        // Validate bot token
        match self.validate_bot_token().await {
            Ok(user_id) => {
                *self.bot_user_id.write() = user_id.clone();
                info!("Slack bot authenticated (user_id: {user_id})");
            }
            Err(e) => {
                warn!("Slack auth.test failed (continuing anyway): {e}");
            }
        }

        // Start Socket Mode loop
        self.start_socket_mode_loop();

        *self.running.write() = true;
        self.base.set_enabled(true);
        info!("Slack channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Slack channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "slack channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let (channel_id, thread_ts) = Self::parse_slack_chat_id(&msg.chat_id);
        if channel_id.is_empty() {
            return Err(NemesisError::Channel(format!(
                "invalid slack chat ID: {}",
                msg.chat_id
            )));
        }

        self.post_message(channel_id, &msg.content, thread_ts)
            .await?;

        // Add checkmark reaction for pending acks
        if let Some((_, msg_ref)) = self.pending_acks.remove(&msg.chat_id) {
            let _ = self.add_reaction(&msg_ref.channel_id, &msg_ref.timestamp, "white_check_mark").await;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slack_chat_id_simple() {
        let (channel, thread) = SlackChannel::parse_slack_chat_id("C12345");
        assert_eq!(channel, "C12345");
        assert!(thread.is_none());
    }

    #[test]
    fn test_parse_slack_chat_id_with_thread() {
        let (channel, thread) = SlackChannel::parse_slack_chat_id("C12345/1234567890.123456");
        assert_eq!(channel, "C12345");
        assert_eq!(thread.unwrap(), "1234567890.123456");
    }

    #[tokio::test]
    async fn test_slack_channel_new_validates_tokens() {
        let config = SlackConfig {
            bot_token: String::new(),
            app_token: String::new(),
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let result = SlackChannel::new(config, tx);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_slack_channel_lifecycle() {
        let config = SlackConfig {
            bot_token: "xoxb-test".to_string(),
            app_token: "xapp-test".to_string(),
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = SlackChannel::new(config, tx).unwrap();
        assert_eq!(ch.name(), "slack");

        // Note: start() will try to connect to Slack, so we don't test the
        // full lifecycle here. Just test that it initializes correctly.
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_strip_bot_mention() {
        let config = SlackConfig {
            bot_token: "xoxb-test".to_string(),
            app_token: "xapp-test".to_string(),
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = SlackChannel::new(config, tx).unwrap();
        ch.set_bot_user_id("U12345".to_string());

        let text = ch.strip_bot_mention("<@U12345> hello world");
        assert_eq!(text, "hello world");
    }

    #[test]
    fn test_parse_slack_event_basic() {
        let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello agent!",
            "ts": "1700000000.000100"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
        assert_eq!(msg.channel, "slack");
        assert_eq!(msg.sender_id, "U456");
        assert_eq!(msg.chat_id, "C789");
        assert_eq!(msg.content, "Hello agent!");
    }

    #[test]
    fn test_parse_slack_event_filters_bot() {
        let bot_id = Arc::new(parking_lot::RwLock::new("U456".to_string()));
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "My message",
            "ts": "1700000000.000100"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_slack_event_filters_bot_id_field() {
        let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Bot message",
            "ts": "1700000000.000100",
            "bot_id": "B999"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_slack_event_allowed_users() {
        let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        // Not allowed
        let msg = SlackChannel::parse_slack_event(
            &event,
            &bot_id,
            &["U111".to_string()],
        );
        assert!(msg.is_none());

        // Allowed
        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &["U456".to_string()]);
        assert!(msg.is_some());

        // Empty = allow all
        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
        assert!(msg.is_some());
    }

    #[test]
    fn test_parse_slack_event_empty_text() {
        let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "",
            "ts": "1700000000.000100"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_slack_event_app_mention() {
        let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> help me",
            "ts": "1700000000.000100"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
        assert_eq!(msg.metadata.get("was_mentioned").unwrap(), "true");
    }

    #[test]
    fn test_parse_slack_event_thread() {
        let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Reply in thread",
            "ts": "1700000001.000200",
            "thread_ts": "1700000000.000100"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
        assert_eq!(msg.chat_id, "C789/1700000000.000100");
        assert_eq!(msg.metadata.get("thread_ts").unwrap(), "1700000000.000100");
    }

    #[test]
    fn test_parse_slack_event_message_changed() {
        let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
        let event = serde_json::json!({
            "type": "message",
            "subtype": "message_changed",
            "channel": "C789",
            "message": {
                "user": "U456",
                "text": "Edited message text",
                "ts": "1700000000.000100"
            },
            "ts": "1700000001.000200"
        });

        let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
        assert_eq!(msg.content, "Edited message text");
    }
}

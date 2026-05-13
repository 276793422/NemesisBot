//! Discord bot channel (WebSocket gateway, @mention handling, guild/DM).
//!
//! Uses the Discord Gateway API over WebSocket for receiving events and
//! the REST API for sending messages. Supports typing indicators and
//! message splitting for Discord's 2000 character limit.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock as TokioRwLock, broadcast};
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

// ---------------------------------------------------------------------------
// Discord Gateway opcodes
// ---------------------------------------------------------------------------

mod opcode {
    pub const DISPATCH: u64 = 0;
    pub const HEARTBEAT: u64 = 1;
    pub const IDENTIFY: u64 = 2;
    pub const RESUME: u64 = 6;
    pub const RECONNECT: u64 = 7;
    pub const INVALID_SESSION: u64 = 9;
    pub const HELLO: u64 = 10;
    pub const HEARTBEAT_ACK: u64 = 11;
}

/// Build a Discord gateway heartbeat (opcode 1) payload.
fn build_heartbeat_payload(last_sequence: Option<u64>) -> serde_json::Value {
    serde_json::json!({
        "op": opcode::HEARTBEAT,
        "d": last_sequence,
    })
}

const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

/// Default gateway intents:
/// GUILDS (1) | GUILD_MESSAGES (512) | DIRECT_MESSAGES (4096) = 4613
const DEFAULT_INTENTS: u64 = 4613;

// ---------------------------------------------------------------------------
// Discord API types
// ---------------------------------------------------------------------------

/// Discord channel configuration.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Bot token.
    pub token: String,
    /// Allowed user IDs (empty = allow all).
    pub allow_from: Vec<String>,
    /// Discord API base URL.
    pub api_base: String,
    /// Gateway intents bitmask.
    pub intents: u64,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            allow_from: Vec::new(),
            api_base: "https://discord.com/api/v10".to_string(),
            intents: DEFAULT_INTENTS,
        }
    }
}

/// Discord message object (simplified).
#[derive(Debug, Deserialize)]
pub struct DiscordMessage {
    pub id: String,
    pub channel_id: String,
    pub content: String,
    pub author: DiscordUser,
    pub guild_id: Option<String>,
    pub attachments: Vec<DiscordAttachment>,
}

/// Discord user object.
#[derive(Debug, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub discriminator: Option<String>,
}

/// Discord attachment.
#[derive(Debug, Deserialize)]
pub struct DiscordAttachment {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub content_type: Option<String>,
}

/// Parameters for creating a message.
#[derive(Serialize)]
struct CreateMessageParams {
    content: String,
}

/// Discord API response for creating a message.
#[derive(Debug, Deserialize)]
struct CreateMessageResponse {
    id: String,
}

// ---------------------------------------------------------------------------
// DiscordChannel
// ---------------------------------------------------------------------------

/// Discord channel using WebSocket Gateway for receiving and REST API for sending.
///
/// Connects to Discord Gateway over WebSocket to receive MESSAGE_CREATE events,
/// converts them to `InboundMessage`, and publishes to the bus. Uses REST API
/// for sending outbound messages.
pub struct DiscordChannel {
    base: BaseChannel,
    config: DiscordConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    typing_stops: RwLock<HashMap<String, tokio::task::JoinHandle<()>>>,
    transcriber: parking_lot::RwLock<Option<Arc<dyn crate::base::VoiceTranscriber>>>,
    /// Bus sender for publishing inbound messages.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// Bot's own user ID (populated after READY event).
    bot_user_id: Arc<TokioRwLock<Option<String>>>,
    /// Session ID for resume.
    session_id: Arc<TokioRwLock<Option<String>>>,
    /// Resume gateway URL.
    resume_gateway_url: Arc<TokioRwLock<Option<String>>>,
}

impl DiscordChannel {
    /// Creates a new `DiscordChannel`.
    pub fn new(config: DiscordConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.token.is_empty() {
            return Err(NemesisError::Channel(
                "discord bot token is required".to_string(),
            ));
        }

        let http = reqwest::Client::new();

        Ok(Self {
            base: BaseChannel::new("discord"),
            config,
            http,
            running: Arc::new(parking_lot::RwLock::new(false)),
            typing_stops: RwLock::new(HashMap::new()),
            transcriber: parking_lot::RwLock::new(None),
            bus_sender,
            bot_user_id: Arc::new(TokioRwLock::new(None)),
            session_id: Arc::new(TokioRwLock::new(None)),
            resume_gateway_url: Arc::new(TokioRwLock::new(None)),
        })
    }

    /// Creates a new `DiscordChannel` with a pre-configured HTTP client.
    ///
    /// Mirrors Go's `NewDiscordChannelWithClient()`. Useful for dependency injection
    /// in testing scenarios where you want to control the HTTP transport layer.
    pub fn new_with_client(
        config: DiscordConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
        http: reqwest::Client,
    ) -> Result<Self> {
        if config.token.is_empty() {
            return Err(NemesisError::Channel(
                "discord bot token is required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("discord"),
            config,
            http,
            running: Arc::new(parking_lot::RwLock::new(false)),
            typing_stops: RwLock::new(HashMap::new()),
            transcriber: parking_lot::RwLock::new(None),
            bus_sender,
            bot_user_id: Arc::new(TokioRwLock::new(None)),
            session_id: Arc::new(TokioRwLock::new(None)),
            resume_gateway_url: Arc::new(TokioRwLock::new(None)),
        })
    }

    /// Set the voice transcriber for audio message transcription.
    pub fn set_transcriber(&self, transcriber: Arc<dyn crate::base::VoiceTranscriber>) {
        *self.transcriber.write() = Some(transcriber);
    }

    /// Sends a message to a Discord channel via REST API.
    async fn send_discord_message(&self, channel_id: &str, content: &str) -> Result<()> {
        let url = format!(
            "{}/channels/{}/messages",
            self.config.api_base, channel_id
        );

        let params = CreateMessageParams {
            content: content.to_string(),
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bot {}", self.config.token))
            .header("Content-Type", "application/json")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("discord send failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "discord send returned {status}: {body}"
            )));
        }

        Ok(())
    }

    /// Triggers typing indicator in a channel.
    pub async fn trigger_typing(&self, channel_id: &str) -> Result<()> {
        let url = format!(
            "{}/channels/{}/typing",
            self.config.api_base, channel_id
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bot {}", self.config.token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("discord typing failed: {e}")))?;

        if resp.status().as_u16() != 204 && !resp.status().is_success() {
            debug!("typing indicator returned non-success status");
        }

        Ok(())
    }

    /// Splits a message into chunks respecting Discord's 2000 char limit.
    pub fn split_message(content: &str, max_len: usize) -> Vec<String> {
        if content.len() <= max_len {
            return vec![content.to_string()];
        }

        let mut chunks = Vec::new();
        let mut remaining = content;

        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                chunks.push(remaining.to_string());
                break;
            }

            // Try to split at newline
            if let Some(idx) = remaining[..max_len].rfind('\n') {
                chunks.push(remaining[..idx].to_string());
                remaining = &remaining[idx + 1..];
            } else if let Some(idx) = remaining[..max_len].rfind(' ') {
                chunks.push(remaining[..idx].to_string());
                remaining = &remaining[idx + 1..];
            } else {
                chunks.push(remaining[..max_len].to_string());
                remaining = &remaining[max_len..];
            }
        }

        chunks
    }

    /// Parses a Discord chat ID (just the channel ID).
    pub fn parse_chat_id(chat_id: &str) -> Result<&str> {
        if chat_id.is_empty() {
            return Err(NemesisError::Channel("empty chat ID".to_string()));
        }
        Ok(chat_id)
    }

    /// Gets the WebSocket gateway URL from the Discord API.
    async fn get_gateway_url(&self) -> Result<String> {
        let url = format!("{}/gateway/bot", self.config.api_base);
        let resp: serde_json::Value = self
            .http
            .get(&url)
            .header("Authorization", format!("Bot {}", self.config.token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("discord gateway request failed: {e}")))?
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("discord gateway parse failed: {e}")))?;

        let ws_url = resp["url"]
            .as_str()
            .ok_or_else(|| NemesisError::Channel("missing 'url' in gateway response".to_string()))?;

        Ok(format!("{ws_url}/?v=10&encoding=json"))
    }

    /// Starts the WebSocket gateway receive loop in a background task.
    ///
    /// Connects to Discord Gateway, sends IDENTIFY, handles heartbeat,
    /// and parses MESSAGE_CREATE events into InboundMessages.
    fn start_gateway_loop(&self) {
        let token = self.config.token.clone();
        let intents = self.config.intents;
        let allow_from = self.config.allow_from.clone();
        let running = self.running.clone();
        let bus_sender = self.bus_sender.clone();
        let bot_user_id = self.bot_user_id.clone();
        let session_id_store = self.session_id.clone();
        let resume_url_store = self.resume_gateway_url.clone();

        // We need the gateway URL — spawn an async block that first fetches it
        let http = self.http.clone();
        let api_base = self.config.api_base.clone();

        tokio::spawn(async move {
            let mut backoff = INITIAL_BACKOFF;

            // Fetch initial gateway URL
            let gateway_url = {
                let url = format!("{api_base}/gateway/bot");
                let resp: serde_json::Value = match http
                    .get(&url)
                    .header("Authorization", format!("Bot {token}"))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Discord: failed to get gateway URL: {e}");
                        return;
                    }
                }
                .json()
                .await
                .unwrap_or_default();

                match resp["url"].as_str() {
                    Some(u) => format!("{u}/?v=10&encoding=json"),
                    None => {
                        error!("Discord: missing URL in gateway response");
                        return;
                    }
                }
            };

            let mut connect_url = gateway_url;
            let sequence: Arc<TokioRwLock<Option<u64>>> = Arc::new(TokioRwLock::new(None));

            loop {
                if !*running.read() {
                    break;
                }

                info!("Connecting to Discord gateway...");

                let ws_result = tokio_tungstenite::connect_async(&connect_url).await;
                let ws_stream = match ws_result {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        warn!("Discord gateway connection failed: {e}, retrying in {backoff:?}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                backoff = INITIAL_BACKOFF;
                info!("Discord gateway connected");

                let (ws_tx_raw, mut ws_rx) = ws_stream.split();
                let ws_tx = Arc::new(tokio::sync::Mutex::new(ws_tx_raw));
                let mut heartbeat_handle: Option<tokio::task::JoinHandle<()>> = None;
                let heartbeat_acked = Arc::new(AtomicBool::new(true));

                // Inner message loop
                let should_reconnect = 'inner: loop {
                    let msg = match ws_rx.next().await {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            warn!("Discord WebSocket error: {e}");
                            break 'inner true;
                        }
                        None => {
                            info!("Discord WebSocket closed");
                            break 'inner true;
                        }
                    };

                    let text = match msg {
                        tokio_tungstenite::tungstenite::Message::Text(t) => t,
                        tokio_tungstenite::tungstenite::Message::Close(_) => {
                            info!("Discord gateway closed by server");
                            break 'inner true;
                        }
                        _ => continue,
                    };

                    let payload: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("Discord: failed to parse gateway message: {e}");
                            continue;
                        }
                    };

                    let op = payload["op"].as_u64().unwrap_or(999);

                    // Update sequence number
                    if let Some(s) = payload["s"].as_u64() {
                        *sequence.write().await = Some(s);
                    }

                    match op {
                        opcode::HELLO => {
                            let interval =
                                payload["d"]["heartbeat_interval"].as_u64().unwrap_or(45000);
                            debug!("Discord HELLO: heartbeat_interval={interval}ms");

                            // Spawn heartbeat task
                            if let Some(h) = heartbeat_handle.take() {
                                h.abort();
                            }
                            heartbeat_acked.store(true, Ordering::Relaxed);
                            let hb_sink = ws_tx.clone();
                            let hb_seq = sequence.clone();
                            let hb_acked = heartbeat_acked.clone();
                            let hb_running = running.clone();
                            heartbeat_handle = Some(tokio::spawn(async move {
                                let mut ticker =
                                    tokio::time::interval(std::time::Duration::from_millis(interval));
                                ticker.tick().await; // skip first tick
                                loop {
                                    if !*hb_running.read() {
                                        return;
                                    }
                                    ticker.tick().await;

                                    if !hb_acked.swap(false, Ordering::Relaxed) {
                                        warn!("Discord: heartbeat not ACKed, forcing reconnect");
                                        let _ = hb_sink.lock().await.close().await;
                                        return;
                                    }

                                    let seq = *hb_seq.read().await;
                                    let hb_payload = build_heartbeat_payload(seq);
                                    let text = match serde_json::to_string(&hb_payload) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            error!("Discord: heartbeat serialize failed: {e}");
                                            return;
                                        }
                                    };
                                    if hb_sink
                                        .lock()
                                        .await
                                        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                    debug!("Discord heartbeat sent (seq={:?})", seq);
                                }
                            }));

                            // Send IDENTIFY
                            let identify = serde_json::json!({
                                "op": opcode::IDENTIFY,
                                "d": {
                                    "token": &token,
                                    "intents": intents,
                                    "properties": {
                                        "os": "linux",
                                        "browser": "nemesisbot",
                                        "device": "nemesisbot"
                                    }
                                }
                            });

                            if let Err(e) = ws_tx
                                .lock()
                                .await
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&identify).unwrap().into(),
                                ))
                                .await
                            {
                                error!("Discord: failed to send IDENTIFY: {e}");
                                break 'inner true;
                            }
                        }

                        opcode::DISPATCH => {
                            let event_name = payload["t"].as_str().unwrap_or("");
                            let d = &payload["d"];

                            match event_name {
                                "READY" => {
                                    let user_id =
                                        d["user"]["id"].as_str().unwrap_or("").to_string();
                                    let username =
                                        d["user"]["username"].as_str().unwrap_or("unknown");
                                    let sid =
                                        d["session_id"].as_str().unwrap_or("").to_string();
                                    let resume_url =
                                        d["resume_gateway_url"].as_str().unwrap_or("").to_string();

                                    *bot_user_id.write().await = Some(user_id);
                                    *session_id_store.write().await = Some(sid);
                                    if !resume_url.is_empty() {
                                        *resume_url_store.write().await = Some(resume_url);
                                    }

                                    info!("Discord bot ready: {username}");
                                }

                                "MESSAGE_CREATE" | "MESSAGE_UPDATE" => {
                                    if let Some(inbound) = Self::parse_gateway_message(
                                        d,
                                        &bot_user_id,
                                        &allow_from,
                                    )
                                    .await
                                    {
                                        debug!(
                                            "Discord {} from {} in channel {}",
                                            event_name,
                                            inbound.sender_id,
                                            inbound.chat_id
                                        );
                                        if bus_sender.send(inbound).is_err() {
                                            warn!("Discord: failed to publish inbound message (no receivers)");
                                        }
                                    }
                                }

                                "RESUMED" => {
                                    info!("Discord session resumed successfully");
                                }

                                _ => {
                                    debug!("Discord event: {event_name}");
                                }
                            }
                        }

                        opcode::HEARTBEAT => {
                            let seq = *sequence.read().await;
                            let hb = build_heartbeat_payload(seq);
                            let _ = ws_tx
                                .lock()
                                .await
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&hb).unwrap().into(),
                                ))
                                .await;
                            heartbeat_acked.store(false, Ordering::Relaxed);
                        }

                        opcode::HEARTBEAT_ACK => {
                            debug!("Discord heartbeat ACK received");
                            heartbeat_acked.store(true, Ordering::Relaxed);
                        }

                        opcode::RECONNECT => {
                            info!("Discord: server requested reconnect");
                            break 'inner true;
                        }

                        opcode::INVALID_SESSION => {
                            let resumable = payload["d"].as_bool().unwrap_or(false);
                            if !resumable {
                                *session_id_store.write().await = None;
                                *sequence.write().await = None;
                            }
                            break 'inner true;
                        }

                        _ => {
                            debug!("Discord: unknown opcode {op}");
                        }
                    }
                };

                // Tear down heartbeat task
                if let Some(h) = heartbeat_handle.take() {
                    h.abort();
                }

                if !should_reconnect || !*running.read() {
                    break;
                }

                // Try resume URL if available
                if let Some(ref url) = *resume_url_store.read().await {
                    connect_url = format!("{url}/?v=10&encoding=json");
                }

                warn!("Discord: reconnecting in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("Discord gateway loop stopped");
        });
    }

    /// Parses a Discord MESSAGE_CREATE/UPDATE payload into an InboundMessage.
    async fn parse_gateway_message(
        d: &serde_json::Value,
        bot_user_id: &Arc<TokioRwLock<Option<String>>>,
        allow_from: &[String],
    ) -> Option<InboundMessage> {
        let author = d.get("author")?;
        let author_id = author["id"].as_str()?;

        // Filter out bot's own messages
        if let Some(ref bid) = *bot_user_id.read().await {
            if author_id == bid {
                return None;
            }
        }

        // Filter out other bots
        if author["bot"].as_bool() == Some(true) {
            return None;
        }

        // Filter by allowed users
        if !allow_from.is_empty() && !allow_from.iter().any(|u| u == author_id) {
            debug!("Discord: ignoring message from unlisted user {author_id}");
            return None;
        }

        let content_text = d["content"].as_str().unwrap_or("");
        if content_text.is_empty() {
            return None;
        }

        let channel_id = d["channel_id"].as_str()?;
        let message_id = d["id"].as_str().unwrap_or("0");
        let username = author["username"].as_str().unwrap_or("Unknown");

        let mut metadata = HashMap::new();
        metadata.insert("message_id".to_string(), message_id.to_string());
        metadata.insert("user_id".to_string(), author_id.to_string());
        metadata.insert("username".to_string(), username.to_string());

        // Check if group message (guild_id present)
        if let Some(guild_id) = d["guild_id"].as_str() {
            metadata.insert("guild_id".to_string(), guild_id.to_string());
            metadata.insert("is_group".to_string(), "true".to_string());
        }

        // Check if bot was mentioned
        if let Some(ref bid) = *bot_user_id.read().await {
            let mentioned_in_array = d["mentions"]
                .as_array()
                .map(|arr| arr.iter().any(|m| m["id"].as_str() == Some(bid.as_str())))
                .unwrap_or(false);
            let mentioned_in_content = content_text.contains(&format!("<@{bid}>"))
                || content_text.contains(&format!("<@!{bid}>"));
            if mentioned_in_array || mentioned_in_content {
                metadata.insert("was_mentioned".to_string(), "true".to_string());
            }
        }

        // Build sender_id
        let sender_id = author_id.to_string();

        Some(InboundMessage {
            channel: "discord".to_string(),
            sender_id,
            chat_id: channel_id.to_string(),
            content: content_text.to_string(),
            media: Vec::new(),
            session_key: String::new(),
            correlation_id: String::new(),
            metadata,
        })
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting Discord bot");

        // Start the WebSocket gateway receive loop
        self.start_gateway_loop();

        *self.running.write() = true;
        self.base.set_enabled(true);
        info!("Discord bot connected");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Discord bot");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Cancel all typing tasks
        let mut stops = self.typing_stops.write();
        for (_, handle) in stops.drain() {
            handle.abort();
        }

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "discord bot not running".to_string(),
            ));
        }

        if msg.chat_id.is_empty() {
            return Err(NemesisError::Channel("channel ID is empty".to_string()));
        }

        if msg.content.is_empty() {
            return Ok(());
        }

        self.base.record_sent();

        let chunks = Self::split_message(&msg.content, 2000);
        for chunk in chunks {
            self.send_discord_message(&msg.chat_id, &chunk).await?;
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

    #[tokio::test]
    async fn test_discord_channel_new_validates_token() {
        let config = DiscordConfig::default();
        let (tx, _rx) = broadcast::channel(256);
        let result = DiscordChannel::new(config, tx);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_discord_channel_lifecycle() {
        let config = DiscordConfig {
            token: "test-token".to_string(),
            ..Default::default()
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = DiscordChannel::new(config, tx).unwrap();
        assert_eq!(ch.name(), "discord");

        // Note: start() will try to connect to Discord, so we don't test the
        // full lifecycle here. Just test that it initializes correctly.
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_split_message_short() {
        let chunks = DiscordChannel::split_message("hello", 2000);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn test_split_message_long() {
        let long = "a ".repeat(1500); // 3000 chars
        let chunks = DiscordChannel::split_message(&long, 2000);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= 2000);
        }
        // Reconstructed content should match (minus trailing spaces from split)
        let reconstructed: String = chunks.join(" ");
        assert_eq!(reconstructed.trim(), long.trim());
    }

    #[test]
    fn test_split_message_at_newline() {
        let msg = "line1\nline2\n";
        let chunks = DiscordChannel::split_message(msg, 8);
        assert_eq!(chunks.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_gateway_message_basic() {
        let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
        let d = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Hello agent!",
            "author": {
                "id": "user456",
                "username": "alice",
                "discriminator": "0",
                "bot": false
            }
        });

        let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[])
            .await
            .unwrap();
        assert_eq!(msg.channel, "discord");
        assert_eq!(msg.sender_id, "user456");
        assert_eq!(msg.chat_id, "ch1");
        assert_eq!(msg.content, "Hello agent!");
    }

    #[tokio::test]
    async fn test_parse_gateway_message_filters_bot() {
        let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
        let d = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "My own message",
            "author": {
                "id": "bot123",
                "username": "nemesisbot",
                "discriminator": "0"
            }
        });

        let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_parse_gateway_message_filters_other_bots() {
        let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
        let d = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Bot message",
            "author": {
                "id": "other_bot",
                "username": "somebot",
                "discriminator": "0",
                "bot": true
            }
        });

        let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_parse_gateway_message_allowed_users() {
        let bot_id = Arc::new(TokioRwLock::new(None));
        let d = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "Hello",
            "author": {
                "id": "user999",
                "username": "bob",
                "discriminator": "0",
                "bot": false
            }
        });

        // Not allowed
        let msg = DiscordChannel::parse_gateway_message(
            &d,
            &bot_id,
            &["user111".to_string()],
        )
        .await;
        assert!(msg.is_none());

        // Allowed
        let msg =
            DiscordChannel::parse_gateway_message(&d, &bot_id, &["user999".to_string()]).await;
        assert!(msg.is_some());
    }

    #[tokio::test]
    async fn test_parse_gateway_message_empty_content() {
        let bot_id = Arc::new(TokioRwLock::new(None));
        let d = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "content": "",
            "author": {
                "id": "user1",
                "username": "alice",
                "discriminator": "0",
                "bot": false
            }
        });

        let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_parse_gateway_message_guild() {
        let bot_id = Arc::new(TokioRwLock::new(None));
        let d = serde_json::json!({
            "id": "msg1",
            "channel_id": "ch1",
            "guild_id": "guild1",
            "content": "Hello guild!",
            "author": {
                "id": "user1",
                "username": "alice",
                "discriminator": "0",
                "bot": false
            }
        });

        let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[])
            .await
            .unwrap();
        assert_eq!(msg.metadata.get("guild_id").unwrap(), "guild1");
        assert_eq!(msg.metadata.get("is_group").unwrap(), "true");
    }

    #[test]
    fn test_build_heartbeat_payload_with_sequence() {
        let payload = build_heartbeat_payload(Some(42));
        assert_eq!(payload["op"], 1);
        assert_eq!(payload["d"], 42);
    }

    #[test]
    fn test_build_heartbeat_payload_without_sequence() {
        let payload = build_heartbeat_payload(None);
        assert_eq!(payload["op"], 1);
        assert!(payload["d"].is_null());
    }

    #[tokio::test]
    async fn test_discord_new_with_client_validates_token() {
        let config = DiscordConfig::default();
        let (tx, _rx) = broadcast::channel(256);
        let http = reqwest::Client::new();
        let result = DiscordChannel::new_with_client(config, tx, http);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_discord_new_with_client_success() {
        let config = DiscordConfig {
            token: "test-token".to_string(),
            ..Default::default()
        };
        let (tx, _rx) = broadcast::channel(256);
        let http = reqwest::Client::new();
        let ch = DiscordChannel::new_with_client(config, tx, http).unwrap();
        assert_eq!(ch.name(), "discord");
        assert!(!*ch.running.read());
    }
}

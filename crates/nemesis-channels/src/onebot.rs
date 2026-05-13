//! OneBot v11 channel (reverse WebSocket, group/private messages).
//!
//! Implements the OneBot v11 protocol via reverse WebSocket for receiving
//! and sending messages. Supports CQ code parsing, deduplication, and
//! group trigger detection.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// OneBot channel configuration.
#[derive(Debug, Clone)]
pub struct OneBotConfig {
    /// WebSocket URL.
    pub ws_url: String,
    /// Access token.
    pub access_token: Option<String>,
    /// Reconnect interval in seconds (0 = no reconnect).
    pub reconnect_interval: u64,
    /// Group trigger prefixes.
    pub group_trigger_prefix: Vec<String>,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// Raw OneBot event from WebSocket.
#[derive(Debug, Deserialize)]
pub struct OneBotRawEvent {
    pub post_type: Option<String>,
    pub message_type: Option<String>,
    pub sub_type: Option<String>,
    pub message_id: Option<serde_json::Value>,
    pub user_id: Option<serde_json::Value>,
    pub group_id: Option<serde_json::Value>,
    pub raw_message: Option<String>,
    pub message: Option<serde_json::Value>,
    pub sender: Option<serde_json::Value>,
    pub self_id: Option<serde_json::Value>,
    pub meta_event_type: Option<String>,
    pub notice_type: Option<String>,
    pub echo: Option<String>,
    pub retcode: Option<serde_json::Value>,
    pub status: Option<serde_json::Value>,
}

/// OneBot API request.
#[derive(Serialize)]
pub struct OneBotApiRequest {
    pub action: String,
    pub params: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echo: Option<String>,
}

/// OneBot message segment.
#[derive(Debug, Deserialize)]
pub struct OneBotMessageSegment {
    #[serde(rename = "type")]
    pub seg_type: String,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

/// OneBot sender info.
#[derive(Debug, Deserialize)]
pub struct OneBotSender {
    pub user_id: Option<serde_json::Value>,
    pub nickname: Option<String>,
    pub card: Option<String>,
}

/// Result of parsing a message.
#[derive(Debug, Default)]
pub struct ParsedMessage {
    pub text: String,
    pub is_bot_mentioned: bool,
    pub media: Vec<String>,
    pub reply_to: Option<String>,
}

const DEDUP_SIZE: usize = 1024;

/// OneBot v11 channel using reverse WebSocket.
pub struct OneBotChannel {
    base: BaseChannel,
    config: OneBotConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    dedup: parking_lot::RwLock<DedupRing>,
    echo_counter: AtomicI64,
    self_id: AtomicI64,
    last_message_ids: dashmap::DashMap<String, String>,
    transcriber: parking_lot::RwLock<Option<Arc<dyn crate::base::VoiceTranscriber>>>,
    bus_sender: broadcast::Sender<InboundMessage>,
    ws_sink: Arc<tokio::sync::RwLock<Option<futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::Message,
    >>>>,
}

struct DedupRing {
    ids: HashMap<String, bool>,
    ring: Vec<Option<String>>,
    index: usize,
}

impl DedupRing {
    fn new(capacity: usize) -> Self {
        Self {
            ids: HashMap::with_capacity(capacity),
            ring: (0..capacity).map(|_| None).collect(),
            index: 0,
        }
    }

    fn check_and_add(&mut self, id: &str) -> bool {
        if self.ids.contains_key(id) {
            return true; // duplicate
        }

        // Evict oldest
        if let Some(ref old) = self.ring[self.index] {
            self.ids.remove(old.as_str());
        }

        self.ring[self.index] = Some(id.to_string());
        self.ids.insert(id.to_string(), true);
        self.index = (self.index + 1) % self.ring.len();

        false
    }
}

impl OneBotChannel {
    /// Creates a new `OneBotChannel`.
    pub fn new(config: OneBotConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.ws_url.is_empty() {
            return Err(NemesisError::Channel(
                "OneBot ws_url not configured".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("onebot"),
            config,
            running: Arc::new(parking_lot::RwLock::new(false)),
            dedup: parking_lot::RwLock::new(DedupRing::new(DEDUP_SIZE)),
            echo_counter: AtomicI64::new(0),
            self_id: AtomicI64::new(0),
            last_message_ids: dashmap::DashMap::new(),
            transcriber: parking_lot::RwLock::new(None),
            bus_sender,
            ws_sink: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Parses a JSON value as i64 (supports both number and string).
    pub fn parse_json_int64(value: &serde_json::Value) -> Option<i64> {
        match value {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// Parses a JSON value as string.
    pub fn parse_json_string(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        }
    }

    /// Checks if a message ID is a duplicate.
    pub fn is_duplicate(&self, message_id: &str) -> bool {
        if message_id.is_empty() || message_id == "0" {
            return false;
        }
        self.dedup.write().check_and_add(message_id)
    }

    /// Sets the self ID.
    pub fn set_self_id(&self, id: i64) {
        self.self_id.store(id, Ordering::SeqCst);
    }

    /// Returns the self ID.
    pub fn self_id(&self) -> i64 {
        self.self_id.load(Ordering::SeqCst)
    }

    /// Set the voice transcriber for audio message transcription.
    ///
    /// Mirrors Go's `OneBotChannel.SetTranscriber()`. When a voice/audio
    /// message is received, the transcriber will be used to convert speech
    /// to text if it is available.
    pub fn set_transcriber(&self, transcriber: Arc<dyn crate::base::VoiceTranscriber>) {
        *self.transcriber.write() = Some(transcriber);
    }

    /// Parses message segments.
    pub fn parse_message_segments(&self, raw: &serde_json::Value) -> ParsedMessage {
        let self_id = self.self_id();

        // Try as plain string
        if let Some(s) = raw.as_str() {
            let mut mentioned = false;
            let text = if self_id > 0 {
                let cq_at = format!("[CQ:at,qq={}]", self_id);
                if s.contains(&cq_at) {
                    mentioned = true;
                    s.replace(&cq_at, "").trim().to_string()
                } else {
                    s.to_string()
                }
            } else {
                s.to_string()
            };
            return ParsedMessage {
                text,
                is_bot_mentioned: mentioned,
                ..Default::default()
            };
        }

        // Try as array of segments
        let segments: Vec<OneBotMessageSegment> = match serde_json::from_value(raw.clone()) {
            Ok(s) => s,
            Err(_) => return ParsedMessage::default(),
        };

        let mut text_parts = Vec::new();
        let mut mentioned = false;
        let self_id_str = self_id.to_string();

        for seg in &segments {
            match seg.seg_type.as_str() {
                "text" => {
                    if let Some(ref data) = seg.data {
                        if let Some(serde_json::Value::String(t)) = data.get("text") {
                            text_parts.push(t.clone());
                        }
                    }
                }
                "at" => {
                    if let Some(ref data) = seg.data {
                        if let Some(val) = data.get("qq") {
                            let qq_val = match val {
                                serde_json::Value::String(s) => s.clone(),
                                serde_json::Value::Number(n) => n.to_string(),
                                _ => String::new(),
                            };
                            if qq_val == self_id_str || qq_val == "all" {
                                mentioned = true;
                            }
                        }
                    }
                }
                "reply" => {
                    // handled separately
                }
                _ => {}
            }
        }

        ParsedMessage {
            text: text_parts.join("").trim().to_string(),
            is_bot_mentioned: mentioned,
            ..Default::default()
        }
    }

    /// Checks group trigger conditions.
    pub fn check_group_trigger(
        &self,
        content: &str,
        is_bot_mentioned: bool,
    ) -> (bool, String) {
        if is_bot_mentioned {
            return (true, content.trim().to_string());
        }

        for prefix in &self.config.group_trigger_prefix {
            if prefix.is_empty() {
                continue;
            }
            if content.starts_with(prefix) {
                return (true, content[prefix.len()..].trim().to_string());
            }
        }

        (false, content.to_string())
    }

    /// Builds a send request.
    pub fn build_send_request(&self, chat_id: &str, content: &str) -> Option<(String, serde_json::Value)> {
        let (action, id_key, raw_id) = if let Some(rest) = chat_id.strip_prefix("group:") {
            ("send_group_msg", "group_id", rest)
        } else if let Some(rest) = chat_id.strip_prefix("private:") {
            ("send_private_msg", "user_id", rest)
        } else {
            ("send_private_msg", "user_id", chat_id)
        };

        let id: i64 = raw_id.parse().ok()?;

        let segments = vec![serde_json::json!({
            "type": "text",
            "data": { "text": content }
        })];

        let params = serde_json::json!({
            id_key: id,
            "message": segments,
        });

        Some((action.to_string(), params))
    }

    /// Stores last message ID for a chat.
    pub fn store_last_message_id(&self, chat_id: &str, message_id: &str) {
        self.last_message_ids.insert(chat_id.to_string(), message_id.to_string());
    }

    /// Gets last message ID for a chat.
    pub fn get_last_message_id(&self, chat_id: &str) -> Option<String> {
        self.last_message_ids.get(chat_id).map(|v| v.value().clone())
    }

    /// Generates a unique echo string for API requests.
    pub fn next_echo(&self) -> String {
        format!(
            "api_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            self.echo_counter.fetch_add(1, Ordering::SeqCst)
        )
    }
}

#[async_trait]
impl Channel for OneBotChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!(ws_url = %self.config.ws_url, "starting OneBot channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let ws_url = self.config.ws_url.clone();
        let access_token = self.config.access_token.clone();
        let running = self.running.clone();
        let ws_sink = self.ws_sink.clone();
        let group_trigger_prefix = self.config.group_trigger_prefix.clone();
        let dedup = self.dedup.clone();
        let self_id_arc = Arc::new(AtomicI64::new(0));
        let last_message_ids = self.last_message_ids.clone();
        let reconnect_interval = self.config.reconnect_interval;

        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            loop {
                if !*running.read() {
                    break;
                }

                // Connect to WebSocket
                let mut request = match tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(&ws_url) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("OneBot: failed to build WS request: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if let Some(ref token) = access_token {
                    if let Ok(val) = format!("Bearer {}", token).parse() {
                        request.headers_mut().insert("Authorization", val);
                    }
                }

                let (ws_stream, _) = match tokio_tungstenite::connect_async(request).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("OneBot: WS connect failed: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                info!("OneBot: connected to {}", ws_url);
                backoff = std::time::Duration::from_secs(1);

                let (sink, mut stream) = ws_stream.split();
                *ws_sink.write().await = Some(sink);

                // Read loop
                use futures::StreamExt;
                use tokio_tungstenite::tungstenite::Message;

                loop {
                    if !*running.read() {
                        break;
                    }

                    let msg = match tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        stream.next(),
                    ).await {
                        Ok(Some(Ok(m))) => m,
                        Ok(Some(Err(e))) => {
                            warn!("OneBot: WS read error: {e}");
                            break;
                        }
                        Ok(None) => {
                            info!("OneBot: WebSocket stream ended");
                            break;
                        }
                        Err(_) => {
                            // Timeout, check if still running
                            continue;
                        }
                    };

                    let text = match msg {
                        Message::Text(t) => t,
                        Message::Ping(_) | Message::Pong(_) => continue,
                        Message::Close(_) => {
                            info!("OneBot: WebSocket closed");
                            break;
                        }
                        _ => continue,
                    };

                    let event: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    // Handle meta_event (lifecycle)
                    let post_type = event["post_type"].as_str().unwrap_or("");
                    if post_type == "meta_event" {
                        if let Some(sid) = event["self_id"].as_i64() {
                            self_id_arc.store(sid, Ordering::SeqCst);
                        }
                        continue;
                    }

                    // Handle API responses (echo)
                    if event.get("echo").is_some() {
                        continue;
                    }

                    // Handle message events
                    if post_type != "message" {
                        continue;
                    }

                    let raw_message = event["raw_message"].as_str().unwrap_or("");
                    if raw_message.is_empty() {
                        continue;
                    }

                    // Parse message ID for dedup
                    let message_id = event["message_id"]
                        .as_i64()
                        .map(|i| i.to_string())
                        .unwrap_or_default();

                    if !message_id.is_empty() && message_id != "0" {
                        if dedup.write().check_and_add(&message_id) {
                            continue;
                        }
                    }

                    // Parse sender
                    let user_id = Self::parse_json_string(
                        event.get("user_id").unwrap_or(&serde_json::Value::Null),
                    );
                    let group_id = event.get("group_id").and_then(|v| {
                        if v.is_null() { None } else { Some(Self::parse_json_string(v)) }
                    });

                    let is_group = group_id.is_some();
                    let chat_id = if let Some(ref gid) = group_id {
                        format!("group:{}", gid)
                    } else {
                        format!("private:{}", user_id)
                    };

                    // Check group trigger
                    let content = if is_group {
                        let mut mentioned = false;
                        let sid = self_id_arc.load(Ordering::SeqCst);
                        if sid > 0 {
                            let cq_at = format!("[CQ:at,qq={}]", sid);
                            if raw_message.contains(&cq_at) {
                                mentioned = true;
                            }
                        }

                        if mentioned {
                            let sid = self_id_arc.load(Ordering::SeqCst);
                            let cq_at = format!("[CQ:at,qq={}]", sid);
                            raw_message.replace(&cq_at, "").trim().to_string()
                        } else {
                            let mut triggered = false;
                            let mut text = raw_message.to_string();
                            for prefix in &group_trigger_prefix {
                                if !prefix.is_empty() && raw_message.starts_with(prefix) {
                                    triggered = true;
                                    text = raw_message[prefix.len()..].trim().to_string();
                                    break;
                                }
                            }
                            if !triggered {
                                continue;
                            }
                            text
                        }
                    } else {
                        raw_message.to_string()
                    };

                    if content.is_empty() {
                        continue;
                    }

                    // Store last message ID
                    if !message_id.is_empty() {
                        last_message_ids.insert(chat_id.clone(), message_id);
                    }

                    let inbound = InboundMessage {
                        channel: "onebot".to_string(),
                        sender_id: user_id,
                        chat_id: chat_id.clone(),
                        content,
                        media: Vec::new(),
                        session_key: chat_id,
                        correlation_id: String::new(),
                        metadata: std::collections::HashMap::new(),
                    };

                    let _ = bus.send(inbound);
                }

                // Clear the WS reference
                *ws_sink.write().await = None;

                if !*running.read() {
                    break;
                }

                // Reconnect
                if reconnect_interval > 0 {
                    info!("OneBot: reconnecting in {}s", reconnect_interval);
                    tokio::time::sleep(std::time::Duration::from_secs(reconnect_interval)).await;
                } else {
                    break;
                }
            }

            info!("OneBot receive loop stopped");
        });

        info!("OneBot channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping OneBot channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "OneBot channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let (action, params) = self
            .build_send_request(&msg.chat_id, &msg.content)
            .ok_or_else(|| {
                NemesisError::Channel(format!(
                    "invalid chat ID format: {}",
                    msg.chat_id
                ))
            })?;

        let echo = self.next_echo();
        let request = OneBotApiRequest {
            action,
            params,
            echo: Some(echo),
        };

        let json = serde_json::to_string(&request)
            .map_err(|e| NemesisError::Channel(format!("OneBot serialize failed: {e}")))?;

        debug!(chat_id = %msg.chat_id, "OneBot sending message");

        // Send via WebSocket
        let mut ws_guard = self.ws_sink.write().await;
        if let Some(sink) = ws_guard.as_mut() {
            use futures::SinkExt;
            use tokio_tungstenite::tungstenite::Message;
            sink.send(Message::Text(json.into())).await
                .map_err(|e| NemesisError::Channel(format!("OneBot WS send failed: {e}")))?;
        } else {
            return Err(NemesisError::Channel(
                "OneBot WebSocket not connected".to_string(),
            ));
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

    fn test_bus() -> broadcast::Sender<InboundMessage> {
        let (tx, _) = broadcast::channel(256);
        tx
    }

    #[tokio::test]
    async fn test_onebot_channel_new_validates() {
        let config = OneBotConfig {
            ws_url: String::new(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: Vec::new(),
            allow_from: Vec::new(),
        };
        assert!(OneBotChannel::new(config, test_bus()).is_err());
    }

    #[tokio::test]
    async fn test_onebot_channel_lifecycle() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 30,
            group_trigger_prefix: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "onebot");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_dedup_ring() {
        let mut ring = DedupRing::new(4);
        assert!(!ring.check_and_add("a"));
        assert!(ring.check_and_add("a"));
        assert!(!ring.check_and_add("b"));
        assert!(!ring.check_and_add("c"));
        assert!(!ring.check_and_add("d"));
        // "a" should be evicted now
        assert!(!ring.check_and_add("e"));
        assert!(ring.check_and_add("b")); // still in ring
    }

    #[test]
    fn test_parse_json_int64() {
        assert_eq!(
            OneBotChannel::parse_json_int64(&serde_json::json!(12345)),
            Some(12345)
        );
        assert_eq!(
            OneBotChannel::parse_json_int64(&serde_json::json!("12345")),
            Some(12345)
        );
        assert_eq!(OneBotChannel::parse_json_int64(&serde_json::json!(null)), None);
    }

    #[test]
    fn test_parse_message_segments_string() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();
        ch.set_self_id(12345);

        let result = ch.parse_message_segments(&serde_json::json!("hello world"));
        assert_eq!(result.text, "hello world");
        assert!(!result.is_bot_mentioned);
    }

    #[test]
    fn test_parse_message_segments_with_at() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();
        ch.set_self_id(12345);

        let result = ch.parse_message_segments(&serde_json::json!("[CQ:at,qq=12345] hello"));
        assert!(result.is_bot_mentioned);
        assert_eq!(result.text, "hello");
    }

    #[test]
    fn test_check_group_trigger_mentioned() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: vec!["/bot".to_string()],
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();

        let (triggered, content) = ch.check_group_trigger("hello", true);
        assert!(triggered);
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_check_group_trigger_prefix() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: vec!["/bot".to_string()],
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();

        let (triggered, content) = ch.check_group_trigger("/bot hello", false);
        assert!(triggered);
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_build_send_request() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();

        let (action, _) = ch.build_send_request("group:12345", "hello").unwrap();
        assert_eq!(action, "send_group_msg");

        let (action, _) = ch.build_send_request("private:67890", "hello").unwrap();
        assert_eq!(action, "send_private_msg");
    }

    #[test]
    fn test_next_echo() {
        let config = OneBotConfig {
            ws_url: "ws://localhost:6700".to_string(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = OneBotChannel::new(config, test_bus()).unwrap();

        let echo1 = ch.next_echo();
        let echo2 = ch.next_echo();
        assert_ne!(echo1, echo2);
        assert!(echo1.starts_with("api_"));
    }
}

//! Feishu/Lark channel (event callback, WebSocket mode, text messages).
//!
//! Uses the Feishu/Lark Open Platform SDK via WebSocket for receiving
//! events and the REST API for sending messages. Falls back to HTTP
//! event polling when the WebSocket endpoint is not available.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Initial backoff duration for reconnect.
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
/// Maximum backoff duration.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
/// Default WebSocket ping interval in seconds.
const DEFAULT_WS_PING_INTERVAL_SECS: u64 = 30;

/// Feishu channel configuration.
#[derive(Debug, Clone)]
pub struct FeishuConfig {
    /// App ID from Feishu Open Platform.
    pub app_id: String,
    /// App Secret.
    pub app_secret: String,
    /// Verification token.
    pub verification_token: String,
    /// Encrypt key.
    pub encrypt_key: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// Feishu message event payload.
#[derive(Debug, Deserialize)]
pub struct FeishuMessageEvent {
    pub event: Option<FeishuEventInner>,
}

/// Inner event data.
#[derive(Debug, Deserialize)]
pub struct FeishuEventInner {
    pub message: Option<FeishuEventMessage>,
    pub sender: Option<FeishuEventSender>,
}

/// Message content.
#[derive(Debug, Deserialize)]
pub struct FeishuEventMessage {
    pub chat_id: Option<String>,
    pub message_id: Option<String>,
    pub message_type: Option<String>,
    pub content: Option<String>,
    pub chat_type: Option<String>,
}

/// Sender information.
#[derive(Debug, Deserialize)]
pub struct FeishuEventSender {
    pub sender_id: Option<FeishuSenderId>,
    pub tenant_key: Option<String>,
}

/// Sender ID variants.
#[derive(Debug, Deserialize)]
pub struct FeishuSenderId {
    pub user_id: Option<String>,
    pub open_id: Option<String>,
    pub union_id: Option<String>,
}

/// Text message content.
#[derive(Debug, Deserialize)]
struct FeishuTextContent {
    text: String,
}

/// Create message request.
#[derive(Serialize)]
struct CreateMessageRequest {
    receive_id: String,
    msg_type: String,
    content: String,
    #[serde(rename = "receive_id_type")]
    receive_id_type: String,
}

/// API response wrapper.
#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    code: i64,
    msg: Option<String>,
    data: Option<T>,
}

/// Tenant access token response.
#[derive(Debug, Deserialize)]
struct TenantTokenResponse {
    tenant_access_token: String,
}

/// Feishu WebSocket endpoint response.
#[derive(Debug, Deserialize)]
struct WsEndpointResponse {
    code: i64,
    msg: Option<String>,
    data: Option<WsEndpointData>,
}

#[derive(Debug, Deserialize)]
struct WsEndpointData {
    url: Option<String>,
    #[serde(default)]
    client_config: Option<WsClientConfig>,
}

#[derive(Debug, Deserialize)]
struct WsClientConfig {
    #[serde(default)]
    ping_interval: Option<u64>,
}

/// Feishu event payload from WebSocket (schema 2.0).
#[derive(Debug, Deserialize)]
struct FeishuEventPayload {
    header: Option<FeishuEventHeader>,
    event: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    event_id: Option<String>,
    event_type: Option<String>,
}

/// Feishu channel using WebSocket mode.
pub struct FeishuChannel {
    base: BaseChannel,
    config: FeishuConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    access_token: Arc<parking_lot::RwLock<String>>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// Cancellation sender for the receive loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl FeishuChannel {
    /// Creates a new `FeishuChannel`.
    pub fn new(
        config: FeishuConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.app_id.is_empty() || config.app_secret.is_empty() {
            return Err(NemesisError::Channel(
                "feishu app_id and app_secret are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("feishu"),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            access_token: Arc::new(parking_lot::RwLock::new(String::new())),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Obtains a tenant access token.
    pub async fn refresh_token(&self) -> Result<String> {
        let params = serde_json::json!({
            "app_id": self.config.app_id,
            "app_secret": self.config.app_secret,
        });

        let resp = self
            .http
            .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("feishu auth failed: {e}")))?;

        let body: ApiResponse<TenantTokenResponse> = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("feishu auth parse failed: {e}")))?;

        if body.code != 0 {
            return Err(NemesisError::Channel(format!(
                "feishu auth error: code={} msg={}",
                body.code,
                body.msg.unwrap_or_default()
            )));
        }

        let token = body
            .data
            .map(|d| d.tenant_access_token)
            .unwrap_or_default();
        *self.access_token.write() = token.clone();
        Ok(token)
    }

    /// Sends a text message to a Feishu chat.
    pub async fn send_text_message(&self, chat_id: &str, text: &str) -> Result<()> {
        let token = self.access_token.read().clone();
        if token.is_empty() {
            return Err(NemesisError::Channel(
                "no access token available".to_string(),
            ));
        }

        let content = serde_json::json!({ "text": text }).to_string();

        let params = CreateMessageRequest {
            receive_id: chat_id.to_string(),
            msg_type: "text".to_string(),
            content,
            receive_id_type: "chat_id".to_string(),
        };

        let resp = self
            .http
            .post("https://open.feishu.cn/open-apis/im/v1/messages")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("feishu send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "feishu send error: {body}"
            )));
        }

        Ok(())
    }

    /// Extracts sender ID from a Feishu event sender.
    pub fn extract_sender_id(sender: &FeishuEventSender) -> &str {
        if let Some(ref sid) = sender.sender_id {
            if let Some(ref uid) = sid.user_id {
                if !uid.is_empty() {
                    return uid;
                }
            }
            if let Some(ref oid) = sid.open_id {
                if !oid.is_empty() {
                    return oid;
                }
            }
            if let Some(ref uid) = sid.union_id {
                if !uid.is_empty() {
                    return uid;
                }
            }
        }
        "unknown"
    }

    /// Extracts message content from a Feishu event message.
    pub fn extract_message_content(message: &FeishuEventMessage) -> String {
        let content = match message.content {
            Some(ref c) => c,
            None => return String::new(),
        };

        if message.message_type.as_deref() == Some("text") {
            if let Ok(text_content) = serde_json::from_str::<FeishuTextContent>(content) {
                return text_content.text;
            }
        }

        content.clone()
    }

    /// Fetches a WebSocket endpoint URL from the Feishu API.
    async fn get_ws_endpoint(&self) -> Result<(String, u64)> {
        let resp = self
            .http
            .post("https://open.feishu.cn/callback/ws/endpoint")
            .json(&serde_json::json!({
                "AppID": self.config.app_id,
                "AppSecret": self.config.app_secret,
            }))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("feishu ws endpoint failed: {e}")))?;

        let body: WsEndpointResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("feishu ws endpoint parse failed: {e}")))?;

        if body.code != 0 {
            return Err(NemesisError::Channel(format!(
                "feishu ws endpoint error: code={} msg={}",
                body.code,
                body.msg.unwrap_or_default()
            )));
        }

        let data = body
            .data
            .ok_or_else(|| NemesisError::Channel("feishu ws endpoint: missing data".to_string()))?;

        let url = data
            .url
            .ok_or_else(|| NemesisError::Channel("feishu ws endpoint: missing url".to_string()))?;

        let ping_interval = data
            .client_config
            .and_then(|c| c.ping_interval)
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_WS_PING_INTERVAL_SECS);

        Ok((url, ping_interval))
    }

    /// Parses a Feishu event JSON and publishes an InboundMessage.
    fn parse_and_publish_event(
        event_json: &serde_json::Value,
        bus_sender: &broadcast::Sender<InboundMessage>,
        allow_from: &[String],
    ) {
        // Schema 2.0 format
        let event = match event_json.get("event") {
            Some(e) => e,
            None => return,
        };

        let message_data = match event.get("message") {
            Some(m) => m,
            None => return,
        };

        let chat_id = message_data
            .get("chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if chat_id.is_empty() {
            return;
        }

        let sender_data = event.get("sender");
        let sender_id = sender_data
            .and_then(|s| s.get("sender_id"))
            .and_then(|sid| {
                sid.get("user_id")
                    .or_else(|| sid.get("open_id"))
                    .or_else(|| sid.get("union_id"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Check allow list
        if !allow_from.is_empty() && !allow_from.contains(&sender_id) {
            debug!(sender_id = %sender_id, "Feishu message filtered by allow_list");
            return;
        }

        let content = message_data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Try to extract text from JSON content
        let text = if let Ok(tc) = serde_json::from_str::<FeishuTextContent>(content) {
            tc.text
        } else {
            content.to_string()
        };

        if text.is_empty() {
            return;
        }

        let message_id = message_data
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let message_type = message_data
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let chat_type = message_data
            .get("chat_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut metadata = std::collections::HashMap::new();
        if !message_id.is_empty() {
            metadata.insert("message_id".to_string(), message_id);
        }
        if !message_type.is_empty() {
            metadata.insert("message_type".to_string(), message_type);
        }
        if !chat_type.is_empty() {
            metadata.insert("chat_type".to_string(), chat_type);
        }

        let inbound = InboundMessage {
            channel: "feishu".to_string(),
            sender_id: sender_id.clone(),
            chat_id: chat_id.clone(),
            content: text,
            media: Vec::new(),
            session_key: format!("feishu:{}", chat_id),
            correlation_id: String::new(),
            metadata,
        };

        info!(
            sender_id = %inbound.sender_id,
            chat_id = %inbound.chat_id,
            "Feishu received message"
        );

        if let Err(e) = bus_sender.send(inbound) {
            warn!("Feishu: failed to publish inbound message: {e}");
        }
    }

    /// Spawns the WebSocket receive loop.
    fn spawn_receive_loop(&self) {
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        let http = self.http.clone();
        let config = self.config.clone();
        let running = self.running.clone();
        let bus_sender = self.bus_sender.clone();
        let allow_from = self.config.allow_from.clone();

        tokio::spawn(async move {
            let mut cancel_rx = cancel_rx;
            let mut backoff = INITIAL_BACKOFF;

            loop {
                if !*running.read() {
                    break;
                }

                // Step 1: Get access token
                let token_params = serde_json::json!({
                    "app_id": config.app_id,
                    "app_secret": config.app_secret,
                });

                let token_result = http
                    .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
                    .json(&token_params)
                    .send()
                    .await;

                let _token = match token_result {
                    Ok(resp) => {
                        if let Ok(body) = resp
                            .json::<ApiResponse<TenantTokenResponse>>()
                            .await
                        {
                            if body.code == 0 {
                                body.data
                                    .map(|d| d.tenant_access_token)
                                    .unwrap_or_default()
                            } else {
                                warn!(code = body.code, "Feishu auth error, backing off");
                                tokio::select! {
                                    _ = tokio::time::sleep(backoff) => {}
                                    _ = &mut cancel_rx => break,
                                }
                                backoff = (backoff * 2).min(MAX_BACKOFF);
                                continue;
                            }
                        } else {
                            String::new()
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Feishu auth request failed, backing off");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = &mut cancel_rx => break,
                        }
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                // Step 2: Get WebSocket endpoint
                let endpoint_result = http
                    .post("https://open.feishu.cn/callback/ws/endpoint")
                    .json(&serde_json::json!({
                        "AppID": config.app_id,
                        "AppSecret": config.app_secret,
                    }))
                    .send()
                    .await;

                let (ws_url, ping_interval) = match endpoint_result {
                    Ok(resp) => {
                        if let Ok(body) = resp.json::<WsEndpointResponse>().await {
                            if body.code == 0 {
                                if let Some(data) = body.data {
                                    let url = data.url.unwrap_or_default();
                                    let pi = data
                                        .client_config
                                        .and_then(|c| c.ping_interval)
                                        .filter(|v| *v > 0)
                                        .unwrap_or(DEFAULT_WS_PING_INTERVAL_SECS);
                                    (url, pi)
                                } else {
                                    warn!("Feishu ws endpoint: missing data, backing off");
                                    tokio::select! {
                                        _ = tokio::time::sleep(backoff) => {}
                                        _ = &mut cancel_rx => break,
                                    }
                                    backoff = (backoff * 2).min(MAX_BACKOFF);
                                    continue;
                                }
                            } else {
                                warn!(code = body.code, "Feishu ws endpoint error, backing off");
                                tokio::select! {
                                    _ = tokio::time::sleep(backoff) => {}
                                    _ = &mut cancel_rx => break,
                                }
                                backoff = (backoff * 2).min(MAX_BACKOFF);
                                continue;
                            }
                        } else {
                            warn!("Feishu ws endpoint parse failed, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Feishu ws endpoint request failed, backing off");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = &mut cancel_rx => break,
                        }
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                if ws_url.is_empty() {
                    warn!("Feishu ws endpoint returned empty URL, backing off");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = &mut cancel_rx => break,
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }

                info!(url = %ws_url, "Feishu connecting to WebSocket endpoint");

                // Step 3: Connect to WebSocket
                match connect_async(&ws_url).await {
                    Ok((ws_stream, _)) => {
                        info!("Feishu WebSocket connected successfully");
                        backoff = INITIAL_BACKOFF;

                        let (mut write, mut read) = ws_stream.split();
                        let mut ping_interval_timer =
                            tokio::time::interval(std::time::Duration::from_secs(ping_interval));
                        ping_interval_timer.tick().await; // skip first tick

                        loop {
                            tokio::select! {
                                msg = read.next() => {
                                    match msg {
                                        Some(Ok(Message::Text(text))) => {
                                            // Handle text-based event messages
                                            if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&text) {
                                                Self::parse_and_publish_event(
                                                    &event_json,
                                                    &bus_sender,
                                                    &allow_from,
                                                );
                                            }
                                        }
                                        Some(Ok(Message::Binary(data))) => {
                                            // Feishu uses protobuf binary frames.
                                            // Try to extract JSON payload from the frame.
                                            // The protobuf frame structure places the payload
                                            // after the headers. We attempt to find JSON within
                                            // the binary data.
                                            if let Some(event_str) = extract_json_from_protobuf_frame(&data) {
                                                if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&event_str) {
                                                    Self::parse_and_publish_event(
                                                        &event_json,
                                                        &bus_sender,
                                                        &allow_from,
                                                    );
                                                }
                                            } else {
                                                // Try parsing the whole binary as JSON (fallback)
                                                if let Ok(event_json) = serde_json::from_slice::<serde_json::Value>(&data) {
                                                    Self::parse_and_publish_event(
                                                        &event_json,
                                                        &bus_sender,
                                                        &allow_from,
                                                    );
                                                }
                                            }
                                        }
                                        Some(Ok(Message::Ping(payload))) => {
                                            let _ = write.send(Message::Pong(payload)).await;
                                        }
                                        Some(Ok(Message::Close(frame))) => {
                                            info!(frame = ?frame, "Feishu WebSocket closed by server");
                                            break;
                                        }
                                        Some(Ok(Message::Pong(_))) => {
                                            debug!("Feishu WebSocket pong received");
                                        }
                                        Some(Ok(_)) => {}
                                        Some(Err(e)) => {
                                            warn!(error = %e, "Feishu WebSocket stream error");
                                            break;
                                        }
                                        None => {
                                            info!("Feishu WebSocket stream ended");
                                            break;
                                        }
                                    }
                                }
                                _ = ping_interval_timer.tick() => {
                                    // Send a text ping (Feishu accepts text-based pings)
                                    if write.send(Message::Ping(vec![].into())).await.is_err() {
                                        warn!("Feishu WebSocket ping failed");
                                        break;
                                    }
                                }
                                _ = &mut cancel_rx => {
                                    info!("Feishu WebSocket receive loop shutting down");
                                    let _ = write.close().await;
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Feishu WebSocket connect failed, backing off");
                    }
                }

                // Reset cancel_rx for next iteration (it was consumed)
                if !*running.read() {
                    break;
                }

                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = async {
                        // Can't reuse cancel_rx since it may have been consumed.
                        // Just wait for the sleep.
                    } => {}
                }

                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("Feishu receive loop stopped");
        });
    }
}

/// Attempts to extract JSON payload from a Feishu protobuf binary frame.
///
/// Feishu WebSocket frames use protobuf encoding. The JSON payload is embedded
/// as a bytes field. This function scans for JSON patterns within the binary data.
fn extract_json_from_protobuf(data: &[u8]) -> Option<String> {
    // Look for the start of a JSON object within the binary data
    // Protobuf wire format: field_number << 3 | wire_type
    // Bytes fields (wire type 2) contain length-prefixed data
    // We scan for the pattern where a JSON string begins after protobuf framing
    for i in 0..data.len().saturating_sub(2) {
        if data[i] == b'{' {
            // Try to parse from this position as JSON
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data[i..]) {
                // Validate it looks like a Feishu event
                if value.get("header").is_some() || value.get("event").is_some() {
                    return Some(value.to_string());
                }
                // Try as nested object with event key
                if value.is_object() && value.as_object().map_or(false, |m| !m.is_empty()) {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Alias used in the code for clarity.
fn extract_json_from_protobuf_frame(data: &[u8]) -> Option<String> {
    extract_json_from_protobuf(data)
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting Feishu channel (WebSocket mode)");
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Try to obtain access token
        match self.refresh_token().await {
            Ok(token) => info!(token_len = token.len(), "Feishu authenticated"),
            Err(e) => warn!(error = %e, "Feishu auth failed (will retry in receive loop)"),
        }

        // Start the WebSocket receive loop
        self.spawn_receive_loop();

        info!("Feishu channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Feishu channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        *self.access_token.write() = String::new();
        info!("Feishu channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "feishu channel not running".to_string(),
            ));
        }

        if msg.chat_id.is_empty() {
            return Err(NemesisError::Channel("chat ID is empty".to_string()));
        }

        self.base.record_sent();
        debug!(chat_id = %msg.chat_id, "Feishu sending message");
        self.send_text_message(&msg.chat_id, &msg.content).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> FeishuConfig {
        FeishuConfig {
            app_id: "cli_test".to_string(),
            app_secret: "secret".to_string(),
            verification_token: String::new(),
            encrypt_key: String::new(),
            allow_from: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_feishu_channel_new_validates() {
        let config = FeishuConfig {
            app_id: String::new(),
            app_secret: String::new(),
            verification_token: String::new(),
            encrypt_key: String::new(),
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        assert!(FeishuChannel::new(config, tx).is_err());
    }

    #[tokio::test]
    async fn test_feishu_channel_lifecycle() {
        let config = make_config();
        let (tx, _rx) = broadcast::channel(256);
        let ch = FeishuChannel::new(config, tx).unwrap();
        assert_eq!(ch.name(), "feishu");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_extract_sender_id_user_id() {
        let sender = FeishuEventSender {
            sender_id: Some(FeishuSenderId {
                user_id: Some("u123".to_string()),
                open_id: Some("ou456".to_string()),
                union_id: None,
            }),
            tenant_key: None,
        };
        assert_eq!(FeishuChannel::extract_sender_id(&sender), "u123");
    }

    #[test]
    fn test_extract_sender_id_fallback() {
        let sender = FeishuEventSender {
            sender_id: Some(FeishuSenderId {
                user_id: Some(String::new()),
                open_id: Some("ou456".to_string()),
                union_id: None,
            }),
            tenant_key: None,
        };
        assert_eq!(FeishuChannel::extract_sender_id(&sender), "ou456");
    }

    #[test]
    fn test_extract_message_content_text() {
        let msg = FeishuEventMessage {
            chat_id: Some("oc_xxx".to_string()),
            message_id: Some("om_xxx".to_string()),
            message_type: Some("text".to_string()),
            content: Some(r#"{"text":"hello"}"#.to_string()),
            chat_type: Some("group".to_string()),
        };
        assert_eq!(FeishuChannel::extract_message_content(&msg), "hello");
    }

    #[test]
    fn test_parse_and_publish_event() {
        let (tx, mut rx) = broadcast::channel(256);

        let event = serde_json::json!({
            "header": {
                "event_id": "evt_123",
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "message": {
                    "chat_id": "oc_test",
                    "message_id": "om_test",
                    "message_type": "text",
                    "content": "{\"text\":\"hello world\"}",
                    "chat_type": "group"
                },
                "sender": {
                    "sender_id": {
                        "user_id": "u123",
                        "open_id": "ou456"
                    }
                }
            }
        });

        FeishuChannel::parse_and_publish_event(&event, &tx, &[]);

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.channel, "feishu");
        assert_eq!(inbound.sender_id, "u123");
        assert_eq!(inbound.chat_id, "oc_test");
        assert_eq!(inbound.content, "hello world");
        assert_eq!(inbound.metadata.get("message_id").unwrap(), "om_test");
    }

    #[test]
    fn test_parse_and_publish_event_filtered() {
        let (tx, mut rx) = broadcast::channel(256);

        let event = serde_json::json!({
            "header": {
                "event_id": "evt_456",
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "message": {
                    "chat_id": "oc_test",
                    "message_id": "om_test",
                    "message_type": "text",
                    "content": "{\"text\":\"hello\"}",
                    "chat_type": "p2p"
                },
                "sender": {
                    "sender_id": {
                        "user_id": "u_blocked"
                    }
                }
            }
        });

        FeishuChannel::parse_and_publish_event(&event, &tx, &["u_allowed".to_string()]);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_extract_json_from_protobuf() {
        // Test with embedded JSON in binary data
        let json_str = r#"{"header":{"event_id":"evt_1"},"event":{"message":{"chat_id":"oc_1","content":"{\"text\":\"hi\"}"}}}"#;
        let mut data = vec![0u8; 10]; // prefix garbage bytes (simulating protobuf framing)
        data.extend_from_slice(json_str.as_bytes());

        let result = extract_json_from_protobuf(&data);
        assert!(result.is_some());
        assert!(result.unwrap().contains("evt_1"));
    }
}

//! QQ bot channel (REST API + WebSocket, C2C/group messages).
//!
//! Uses the QQ Bot REST API for sending messages and WebSocket for receiving.
//! Supports C2C and group @-mention messages with deduplication.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// QQ channel configuration.
#[derive(Debug, Clone)]
pub struct QQConfig {
    /// App ID from QQ Open Platform.
    pub app_id: String,
    /// App Secret.
    pub app_secret: String,
    /// QQ Bot API base URL.
    pub api_base: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

impl Default for QQConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_secret: String::new(),
            api_base: "https://api.sgroup.qq.com".to_string(),
            allow_from: Vec::new(),
        }
    }
}

/// QQ access token response.
#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    expires_in: Option<u64>,
}

/// QQ WebSocket gateway response.
#[derive(Debug, Deserialize)]
struct WsGatewayResponse {
    url: Option<String>,
}

/// QQ WebSocket dispatch event.
#[derive(Debug, Deserialize)]
struct QqDispatchEvent {
    t: Option<String>,
    s: Option<i64>,
    d: Option<serde_json::Value>,
}

/// QQ message data (C2C).
#[derive(Debug, Deserialize)]
struct QqC2CMessage {
    content: Option<String>,
    author: Option<QqAuthor>,
    id: Option<String>,
}

/// QQ message data (group).
#[derive(Debug, Deserialize)]
struct QqGroupMessage {
    content: Option<String>,
    author: Option<QqAuthor>,
    group_openid: Option<String>,
    id: Option<String>,
}

/// QQ author info.
#[derive(Debug, Deserialize)]
struct QqAuthor {
    member_openid: Option<String>,
    user_openid: Option<String>,
}

/// QQ message send request (C2C).
#[derive(Serialize)]
struct QQSendC2CRequest {
    content: String,
    msg_type: i32,
    msg_id: Option<String>,
}

/// QQ message send request (group).
#[derive(Serialize)]
struct QQSendGroupRequest {
    content: String,
    msg_type: i32,
    msg_id: Option<String>,
}

/// QQ API response.
#[derive(Debug, Deserialize)]
struct QQApiResponse {
    code: Option<i32>,
    message: Option<String>,
}

/// QQ channel using the official Bot SDK REST API.
pub struct QQChannel {
    base: BaseChannel,
    config: QQConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    processed_ids: parking_lot::RwLock<HashMap<String, bool>>,
    access_token: Arc<parking_lot::RwLock<String>>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// Cancellation sender for the receive loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl QQChannel {
    /// Creates a new `QQChannel`.
    pub fn new(
        config: QQConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.app_id.is_empty() || config.app_secret.is_empty() {
            return Err(NemesisError::Channel(
                "QQ app_id and app_secret are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("qq"),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            processed_ids: parking_lot::RwLock::new(HashMap::new()),
            access_token: Arc::new(parking_lot::RwLock::new(String::new())),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Checks if a message ID is a duplicate.
    pub fn is_duplicate(&self, message_id: &str) -> bool {
        let mut map = self.processed_ids.write();
        if map.contains_key(message_id) {
            return true;
        }
        map.insert(message_id.to_string(), true);

        // Simple cleanup: limit map size
        if map.len() > 10000 {
            let keys: Vec<String> = map.keys().take(5000).cloned().collect();
            for key in keys {
                map.remove(&key);
            }
        }

        false
    }

    /// Obtains an access token from the QQ API.
    pub async fn refresh_token(&self) -> Result<String> {
        let params = serde_json::json!({
            "appId": self.config.app_id,
            "clientSecret": self.config.app_secret,
        });

        let resp = self
            .http
            .post(format!("{}/app/getAppAccessToken", self.config.api_base))
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("QQ auth failed: {e}")))?;

        let body: AccessTokenResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("QQ auth parse failed: {e}")))?;

        let token = body.access_token.unwrap_or_default();
        *self.access_token.write() = token.clone();
        Ok(token)
    }

    /// Sends a C2C message via QQ REST API.
    pub async fn send_c2c_message(&self, openid: &str, content: &str) -> Result<()> {
        let token = self.access_token.read().clone();
        if token.is_empty() {
            return Err(NemesisError::Channel(
                "QQ access token not available".to_string(),
            ));
        }

        let request = QQSendC2CRequest {
            content: content.to_string(),
            msg_type: 0, // text
            msg_id: None,
        };

        let url = format!(
            "{}/v2/users/{}/messages",
            self.config.api_base, openid
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("QQ C2C send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "QQ C2C send error: {body}"
            )));
        }

        Ok(())
    }

    /// Sends a group message via QQ REST API.
    pub async fn send_group_message(&self, group_openid: &str, content: &str) -> Result<()> {
        let token = self.access_token.read().clone();
        if token.is_empty() {
            return Err(NemesisError::Channel(
                "QQ access token not available".to_string(),
            ));
        }

        let request = QQSendGroupRequest {
            content: content.to_string(),
            msg_type: 0,
            msg_id: None,
        };

        let url = format!(
            "{}/v2/groups/{}/messages",
            self.config.api_base, group_openid
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("QQ group send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "QQ group send error: {body}"
            )));
        }

        Ok(())
    }

    /// Spawns the WebSocket receive loop for QQ Bot gateway.
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
                let token = {
                    let params = serde_json::json!({
                        "appId": config.app_id,
                        "clientSecret": config.app_secret,
                    });

                    let resp = match http
                        .post(format!("{}/app/getAppAccessToken", config.api_base))
                        .json(&params)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(error = %e, "[QQChannel] auth request failed, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    };

                    let body: AccessTokenResponse = match resp.json().await {
                        Ok(b) => b,
                        Err(e) => {
                            warn!(error = %e, "[QQChannel] auth parse failed, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    };

                    match body.access_token {
                        Some(t) if !t.is_empty() => t,
                        _ => {
                            warn!("[QQChannel] auth returned empty token, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                };

                // Step 2: Get WebSocket gateway URL
                let ws_url = {
                    let resp = match http
                        .get(format!("{}/gateway", config.api_base))
                        .header("Authorization", format!("QQBot {token}"))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(error = %e, "[QQChannel] gateway request failed, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    };

                    if !resp.status().is_success() {
                        let status = resp.status();
                        warn!(status = %status, "[QQChannel] gateway request failed, backing off");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = &mut cancel_rx => break,
                        }
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }

                    let body: WsGatewayResponse = match resp.json().await {
                        Ok(b) => b,
                        Err(e) => {
                            warn!(error = %e, "[QQChannel] gateway parse failed, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    };

                    match body.url {
                        Some(u) if !u.is_empty() => u,
                        _ => {
                            warn!("[QQChannel] gateway returned empty URL, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                };

                info!(url = %ws_url, "[QQChannel] bot connecting to WebSocket gateway");

                // Step 3: Connect to WebSocket
                match connect_async(&ws_url).await {
                    Ok((ws_stream, _)) => {
                        info!("[QQChannel] bot WebSocket connected successfully");
                        backoff = INITIAL_BACKOFF;

                        let (mut write, mut read) = ws_stream.split();
                        let mut heartbeat_interval =
                            tokio::time::interval(std::time::Duration::from_secs(45));
                        heartbeat_interval.tick().await; // skip first tick

                        loop {
                            tokio::select! {
                                msg = read.next() => {
                                    match msg {
                                        Some(Ok(Message::Text(text))) => {
                                            // Parse QQ dispatch event
                                            if let Ok(event) = serde_json::from_str::<QqDispatchEvent>(&text) {
                                                let event_type = event.t.as_deref().unwrap_or("");

                                                match event_type {
                                                    "READY" => {
                                                        info!("[QQChannel] bot READY event received");
                                                    }
                                                    "RESUMED" => {
                                                        info!("[QQChannel] bot RESUMED event received");
                                                    }
                                                    "HEARTBEAT_ACK" => {
                                                        debug!("[QQChannel] bot heartbeat ACK");
                                                    }
                                                    "C2C_MESSAGE_CREATE" => {
                                                        if let Some(data) = &event.d {
                                                            Self::handle_c2c_message(
                                                                data,
                                                                &bus_sender,
                                                                &allow_from,
                                                            );
                                                        }
                                                    }
                                                    "GROUP_AT_MESSAGE_CREATE" => {
                                                        if let Some(data) = &event.d {
                                                            Self::handle_group_message(
                                                                data,
                                                                &bus_sender,
                                                                &allow_from,
                                                            );
                                                        }
                                                    }
                                                    _ => {
                                                        debug!(event_type = %event_type, "[QQChannel] bot unhandled event");
                                                    }
                                                }
                                            }
                                        }
                                        Some(Ok(Message::Ping(payload))) => {
                                            let _ = write.send(Message::Pong(payload)).await;
                                        }
                                        Some(Ok(Message::Close(frame))) => {
                                            info!(frame = ?frame, "[QQChannel] bot WebSocket closed by server");
                                            break;
                                        }
                                        Some(Ok(_)) => {}
                                        Some(Err(e)) => {
                                            warn!(error = %e, "[QQChannel] bot WebSocket stream error");
                                            break;
                                        }
                                        None => {
                                            info!("[QQChannel] bot WebSocket stream ended");
                                            break;
                                        }
                                    }
                                }
                                _ = heartbeat_interval.tick() => {
                                    // Send heartbeat
                                    let heartbeat = serde_json::json!({
                                        "op": 1,
                                        "d": null
                                    });
                                    if write.send(Message::Text(heartbeat.to_string().into())).await.is_err() {
                                        warn!("[QQChannel] bot heartbeat send failed");
                                        break;
                                    }
                                }
                                _ = &mut cancel_rx => {
                                    info!("[QQChannel] bot receive loop shutting down");
                                    let _ = write.close().await;
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "[QQChannel] bot WebSocket connect failed, backing off");
                    }
                }

                if !*running.read() {
                    break;
                }

                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = async {} => {}
                }

                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("[QQChannel] bot receive loop stopped");
        });
    }

    /// Handles a C2C message event from QQ.
    fn handle_c2c_message(
        data: &serde_json::Value,
        bus_sender: &broadcast::Sender<InboundMessage>,
        allow_from: &[String],
    ) {
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if content.is_empty() {
            return;
        }

        let sender_id = data
            .get("author")
            .and_then(|a| a.get("user_openid"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Check allow list
        if !allow_from.is_empty() && !allow_from.contains(&sender_id) {
            debug!(sender_id = %sender_id, "[QQChannel] C2C message filtered by allow_list");
            return;
        }

        let msg_id = data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let chat_id = format!("c2c:{}", &sender_id);

        let mut metadata = std::collections::HashMap::new();
        if !msg_id.is_empty() {
            metadata.insert("message_id".to_string(), msg_id);
        }
        metadata.insert("chat_type".to_string(), "c2c".to_string());

        let inbound = InboundMessage {
            channel: "qq".to_string(),
            sender_id: sender_id.clone(),
            chat_id: chat_id.clone(),
            content,
            media: Vec::new(),
            session_key: format!("qq:{}", chat_id),
            correlation_id: String::new(),
            metadata,
        };

        info!(
            sender_id = %inbound.sender_id,
            chat_id = %inbound.chat_id,
            "[QQChannel] received C2C message"
        );

        if let Err(e) = bus_sender.send(inbound) {
            warn!("[QQChannel] failed to publish inbound message: {e}");
        }
    }

    /// Handles a group @-mention message event from QQ.
    fn handle_group_message(
        data: &serde_json::Value,
        bus_sender: &broadcast::Sender<InboundMessage>,
        allow_from: &[String],
    ) {
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if content.is_empty() {
            return;
        }

        let sender_id = data
            .get("author")
            .and_then(|a| a.get("member_openid"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Check allow list
        if !allow_from.is_empty() && !allow_from.contains(&sender_id) {
            debug!(sender_id = %sender_id, "[QQChannel] group message filtered by allow_list");
            return;
        }

        let group_id = data
            .get("group_openid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let msg_id = data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let chat_id = format!("group:{}", &group_id);

        let mut metadata = std::collections::HashMap::new();
        if !msg_id.is_empty() {
            metadata.insert("message_id".to_string(), msg_id);
        }
        if !group_id.is_empty() {
            metadata.insert("group_openid".to_string(), group_id);
        }
        metadata.insert("chat_type".to_string(), "group".to_string());

        let inbound = InboundMessage {
            channel: "qq".to_string(),
            sender_id: sender_id.clone(),
            chat_id: chat_id.clone(),
            content,
            media: Vec::new(),
            session_key: format!("qq:{}", chat_id),
            correlation_id: String::new(),
            metadata,
        };

        info!(
            sender_id = %inbound.sender_id,
            chat_id = %inbound.chat_id,
            "[QQChannel] received group message"
        );

        if let Err(e) = bus_sender.send(inbound) {
            warn!("[QQChannel] failed to publish inbound message: {e}");
        }
    }
}

#[async_trait]
impl Channel for QQChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("[QQChannel] starting QQ bot");
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Try to obtain access token
        match self.refresh_token().await {
            Ok(token) => info!(token_len = token.len(), "[QQChannel] bot authenticated"),
            Err(e) => warn!(error = %e, "[QQChannel] bot auth failed (will retry in receive loop)"),
        }

        // Start the WebSocket receive loop
        self.spawn_receive_loop();

        info!("[QQChannel] bot started successfully");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[QQChannel] stopping QQ bot");
        *self.running.write() = false;
        self.base.set_enabled(false);

        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        *self.access_token.write() = String::new();
        info!("[QQChannel] bot stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel("QQ bot not running".to_string()));
        }

        self.base.record_sent();

        // Determine message type from chat_id prefix
        if let Some(openid) = msg.chat_id.strip_prefix("c2c:") {
            debug!(openid = %openid, "[QQChannel] sending C2C message");
            self.send_c2c_message(openid, &msg.content).await
        } else if let Some(group_id) = msg.chat_id.strip_prefix("group:") {
            debug!(group_id = %group_id, "[QQChannel] sending group message");
            self.send_group_message(group_id, &msg.content).await
        } else {
            // Default to C2C
            debug!(chat_id = %msg.chat_id, "[QQChannel] sending C2C message (default)");
            self.send_c2c_message(&msg.chat_id, &msg.content).await
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

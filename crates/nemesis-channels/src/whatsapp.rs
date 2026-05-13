//! WhatsApp bridge channel (HTTP/WebSocket connection).
//!
//! Connects to a WhatsApp bridge (e.g. whatsapp-web.js or baileys bridge)
//! via HTTP for sending and WebSocket for receiving. Falls back to
//! a simple HTTP POST/GET pattern when WebSocket is unavailable.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Initial backoff duration for reconnect.
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
/// Maximum backoff duration.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);

/// WhatsApp channel configuration.
#[derive(Debug, Clone)]
pub struct WhatsAppConfig {
    /// Bridge URL (HTTP or WebSocket).
    pub bridge_url: String,
    /// API key for bridge authentication.
    pub api_key: Option<String>,
    /// Allowed sender IDs (empty = allow all).
    pub allow_from: Vec<String>,
}

/// WhatsApp inbound message format.
#[derive(Debug, Deserialize)]
pub struct WhatsAppInboundMessage {
    #[serde(rename = "type")]
    pub msg_type: Option<String>,
    pub from: Option<String>,
    pub chat: Option<String>,
    pub content: Option<String>,
    pub id: Option<String>,
    pub from_name: Option<String>,
    pub media: Option<Vec<String>>,
}

/// WhatsApp outbound message format.
#[derive(Serialize)]
struct WhatsAppOutboundMessage {
    #[serde(rename = "type")]
    msg_type: String,
    to: String,
    content: String,
}

/// Bridge API response.
#[derive(Debug, Deserialize)]
struct BridgeResponse {
    success: Option<bool>,
    error: Option<String>,
}

/// WhatsApp channel using a bridge connection.
pub struct WhatsAppChannel {
    base: BaseChannel,
    config: WhatsAppConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    outbound_queue: parking_lot::RwLock<Vec<OutboundMessage>>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// Cancellation sender for the receive loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl WhatsAppChannel {
    /// Creates a new `WhatsAppChannel`.
    pub fn new(
        config: WhatsAppConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.bridge_url.is_empty() {
            return Err(NemesisError::Channel(
                "whatsapp bridge_url is required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("whatsapp"),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            outbound_queue: parking_lot::RwLock::new(Vec::new()),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Returns the bridge URL.
    pub fn bridge_url(&self) -> &str {
        &self.config.bridge_url
    }

    /// Processes an inbound message from the bridge.
    pub fn process_inbound(&self, msg: &WhatsAppInboundMessage) -> Option<(String, String, String)> {
        let msg_type = msg.msg_type.as_deref().unwrap_or("");
        if msg_type != "message" {
            return None;
        }

        let sender_id = msg.from.as_deref().unwrap_or("unknown");
        let chat_id = msg.chat.as_deref().unwrap_or(sender_id);
        let content = msg.content.as_deref().unwrap_or("");

        Some((
            sender_id.to_string(),
            chat_id.to_string(),
            content.to_string(),
        ))
    }

    /// Drains all queued outbound messages.
    pub fn drain_outbound(&self) -> Vec<OutboundMessage> {
        self.outbound_queue.write().drain(..).collect()
    }

    /// Sends a message via the bridge HTTP API.
    pub async fn send_via_bridge(&self, to: &str, content: &str) -> Result<()> {
        let msg = WhatsAppOutboundMessage {
            msg_type: "text".to_string(),
            to: to.to_string(),
            content: content.to_string(),
        };

        let url = format!("{}/send", self.config.bridge_url.trim_end_matches('/'));
        let mut req = self.http.post(&url).json(&msg);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("whatsapp bridge send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "whatsapp bridge error: {body}"
            )));
        }

        Ok(())
    }

    /// Starts a polling loop to receive messages from the bridge.
    fn spawn_receive_loop(&self) {
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        let base_url = self.config.bridge_url.trim_end_matches('/').to_string();
        let api_key = self.config.api_key.clone();
        let http = self.http.clone();
        let running = self.running.clone();
        let bus_sender = self.bus_sender.clone();
        let allow_from = self.config.allow_from.clone();

        tokio::spawn(async move {
            let mut cancel_rx = cancel_rx;
            let url = format!("{base_url}/receive");
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut backoff = INITIAL_BACKOFF;
            let mut consecutive_errors = 0u32;

            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("WhatsApp receive loop shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        if !*running.read() {
                            break;
                        }

                        let mut req = http.get(&url);
                        if let Some(ref key) = api_key {
                            req = req.header("Authorization", format!("Bearer {key}"));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => {
                                consecutive_errors = 0;
                                if backoff > INITIAL_BACKOFF {
                                    backoff = INITIAL_BACKOFF;
                                }

                                if let Ok(messages) = resp.json::<Vec<WhatsAppInboundMessage>>().await {
                                    for msg in &messages {
                                        // Only process message type
                                        let msg_type = msg.msg_type.as_deref().unwrap_or("");
                                        if msg_type != "message" {
                                            continue;
                                        }

                                        let sender_id = msg.from.as_deref().unwrap_or("unknown").to_string();

                                        // Check allow list
                                        if !allow_from.is_empty() && !allow_from.contains(&sender_id) {
                                            debug!(sender_id = %sender_id, "WhatsApp message filtered by allow_list");
                                            continue;
                                        }

                                        let chat_id = msg.chat.as_deref().unwrap_or(&sender_id).to_string();
                                        let content = msg.content.as_deref().unwrap_or("").to_string();

                                        if content.is_empty() {
                                            continue;
                                        }

                                        let mut metadata = std::collections::HashMap::new();
                                        if let Some(ref id) = msg.id {
                                            metadata.insert("message_id".to_string(), id.clone());
                                        }
                                        if let Some(ref name) = msg.from_name {
                                            metadata.insert("from_name".to_string(), name.clone());
                                        }

                                        let inbound = InboundMessage {
                                            channel: "whatsapp".to_string(),
                                            sender_id: sender_id.clone(),
                                            chat_id: chat_id.clone(),
                                            content,
                                            media: Vec::new(),
                                            session_key: format!("whatsapp:{}", chat_id),
                                            correlation_id: String::new(),
                                            metadata,
                                        };

                                        info!(
                                            sender_id = %inbound.sender_id,
                                            chat_id = %inbound.chat_id,
                                            "WhatsApp received message from bridge"
                                        );

                                        if let Err(e) = bus_sender.send(inbound) {
                                            warn!("WhatsApp: failed to publish inbound message: {e}");
                                        }
                                    }
                                }
                            }
                            Ok(resp) => {
                                consecutive_errors += 1;
                                warn!(status = %resp.status(), "WhatsApp bridge poll returned error");
                                if consecutive_errors > 3 {
                                    interval = tokio::time::interval(backoff);
                                    backoff = (backoff * 2).min(MAX_BACKOFF);
                                    interval.tick().await; // skip first tick
                                }
                            }
                            Err(e) => {
                                consecutive_errors += 1;
                                warn!(error = %e, "WhatsApp bridge poll failed");
                                if consecutive_errors > 3 {
                                    interval = tokio::time::interval(backoff);
                                    backoff = (backoff * 2).min(MAX_BACKOFF);
                                    interval.tick().await; // skip first tick
                                }
                            }
                        }
                    }
                }
            }

            info!("WhatsApp receive loop stopped");
        });
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!(url = %self.config.bridge_url, "starting WhatsApp channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Start the receive loop
        self.spawn_receive_loop();

        info!("WhatsApp channel connected");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping WhatsApp channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Cancel the receive loop
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        self.outbound_queue.write().clear();
        info!("WhatsApp channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "whatsapp connection not established".to_string(),
            ));
        }

        self.base.record_sent();

        debug!(chat_id = %msg.chat_id, "WhatsApp channel sending message");

        // Try to send via bridge HTTP API
        match self.send_via_bridge(&msg.chat_id, &msg.content).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // Fall back to queue if bridge is unavailable
                warn!(error = %e, "WhatsApp bridge send failed, queueing message");
                self.outbound_queue.write().push(msg);
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_whatsapp_channel_new_validates_url() {
        let config = WhatsAppConfig {
            bridge_url: String::new(),
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        assert!(WhatsAppChannel::new(config, tx).is_err());
    }

    #[tokio::test]
    async fn test_whatsapp_channel_lifecycle() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:8080".to_string(),
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();
        assert_eq!(ch.name(), "whatsapp");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[tokio::test]
    async fn test_whatsapp_send_queues_on_bridge_failure() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:19999".to_string(), // unreachable
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "whatsapp".to_string(),
            chat_id: "12345".to_string(),
            content: "Hello".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap(); // should succeed (queued on bridge failure)

        let outbound = ch.drain_outbound();
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].content, "Hello");
    }

    #[test]
    fn test_process_inbound_message() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:8080".to_string(),
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();

        let msg = WhatsAppInboundMessage {
            msg_type: Some("message".to_string()),
            from: Some("+1234567890".to_string()),
            chat: Some("+1234567890".to_string()),
            content: Some("Hello".to_string()),
            id: Some("msg-1".to_string()),
            from_name: Some("John".to_string()),
            media: None,
        };

        let (sender, chat, content) = ch.process_inbound(&msg).unwrap();
        assert_eq!(sender, "+1234567890");
        assert_eq!(content, "Hello");
    }

    #[test]
    fn test_process_inbound_non_message() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:8080".to_string(),
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();

        let msg = WhatsAppInboundMessage {
            msg_type: Some("receipt".to_string()),
            from: None,
            chat: None,
            content: None,
            id: None,
            from_name: None,
            media: None,
        };

        assert!(ch.process_inbound(&msg).is_none());
    }

    // ---- New tests ----

    #[test]
    fn test_whatsapp_config_fields() {
        let config = WhatsAppConfig {
            bridge_url: "http://bridge:8080".into(),
            api_key: Some("key123".into()),
            allow_from: vec!["+1234567890".into()],
        };
        assert_eq!(config.bridge_url, "http://bridge:8080");
        assert_eq!(config.api_key.as_deref(), Some("key123"));
        assert_eq!(config.allow_from.len(), 1);
    }

    #[test]
    fn test_process_inbound_with_media() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:8080".into(),
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();

        let msg = WhatsAppInboundMessage {
            msg_type: Some("message".into()),
            from: Some("+111".into()),
            chat: Some("+111".into()),
            content: Some("See this image".into()),
            id: Some("msg-media".into()),
            from_name: None,
            media: Some(WhatsAppMedia {
                media_type: "image".into(),
                url: "http://bridge/media/123".into(),
                mime_type: Some("image/jpeg".into()),
            }),
        };
        let result = ch.process_inbound(&msg);
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_whatsapp_double_stop() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:8080".into(),
            api_key: None,
            allow_from: Vec::new(),
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();
        ch.start().await.unwrap();
        ch.stop().await.unwrap();
        ch.stop().await.unwrap(); // double stop should be fine
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_process_inbound_allow_from_filter() {
        let config = WhatsAppConfig {
            bridge_url: "http://localhost:8080".into(),
            api_key: None,
            allow_from: vec!["+999".into()],
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = WhatsAppChannel::new(config, tx).unwrap();

        let msg = WhatsAppInboundMessage {
            msg_type: Some("message".into()),
            from: Some("+888".into()),
            chat: Some("+888".into()),
            content: Some("Hello".into()),
            id: Some("id".into()),
            from_name: None,
            media: None,
        };
        assert!(ch.process_inbound(&msg).is_none()); // not in allow_from
    }
}

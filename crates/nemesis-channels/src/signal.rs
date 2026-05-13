//! Signal channel (signal-cli-rest-api REST, envelope processing).
//!
//! Uses the signal-cli-rest-api for sending and receiving Signal messages
//! via long polling on the /v1/receive endpoint.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Signal channel configuration.
#[derive(Debug, Clone)]
pub struct SignalConfig {
    /// signal-cli-rest-api base URL.
    pub api_url: String,
    /// Signal phone number (with country code).
    pub phone_number: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// Signal envelope (incoming message).
#[derive(Debug, Deserialize)]
pub struct SignalEnvelope {
    pub envelope: Option<SignalEnvelopeInner>,
}

/// Inner envelope data.
#[derive(Debug, Deserialize)]
pub struct SignalEnvelopeInner {
    pub source: Option<String>,
    pub source_number: Option<String>,
    pub source_uuid: Option<String>,
    pub timestamp: Option<i64>,
    pub data_message: Option<SignalDataMessage>,
    pub sync_message: Option<serde_json::Value>,
}

/// Signal data message.
#[derive(Debug, Deserialize)]
pub struct SignalDataMessage {
    pub timestamp: Option<i64>,
    pub message: Option<String>,
    pub group_info: Option<SignalGroupInfo>,
}

/// Signal group info.
#[derive(Debug, Deserialize)]
pub struct SignalGroupInfo {
    pub group_id: Option<String>,
    pub name: Option<String>,
}

/// Signal send request.
#[derive(Serialize)]
struct SignalSendRequest {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
}

/// Signal channel using signal-cli-rest-api.
pub struct SignalChannel {
    base: BaseChannel,
    config: SignalConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    processed_timestamps: parking_lot::RwLock<HashMap<i64, bool>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl SignalChannel {
    /// Creates a new `SignalChannel`.
    pub fn new(config: SignalConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.api_url.is_empty() || config.phone_number.is_empty() {
            return Err(NemesisError::Channel(
                "signal api_url and phone_number are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("signal"),
            config,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| NemesisError::Channel(format!("HTTP client creation failed: {e}")))?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            processed_timestamps: parking_lot::RwLock::new(HashMap::new()),
            bus_sender,
        })
    }

    /// Returns the receive URL for long polling.
    pub fn receive_url(&self) -> String {
        format!(
            "{}/v1/receive/{}",
            self.config.api_url, self.config.phone_number
        )
    }

    /// Processes a received envelope.
    pub fn process_envelope(&self, envelope: &SignalEnvelopeInner) -> Option<(String, String, String)> {
        let data_msg = envelope.data_message.as_ref()?;
        let content = data_msg.message.as_deref().unwrap_or("");
        if content.is_empty() {
            return None;
        }

        let sender_id = envelope.source_number.as_deref().unwrap_or("unknown");

        let chat_id = if let Some(ref group_info) = data_msg.group_info {
            group_info.group_id.as_deref().unwrap_or(sender_id)
        } else {
            sender_id
        };

        Some((sender_id.to_string(), chat_id.to_string(), content.to_string()))
    }

    /// Checks if a timestamp has already been processed.
    pub fn is_duplicate(&self, timestamp: i64) -> bool {
        let mut map = self.processed_timestamps.write();
        if map.contains_key(&timestamp) {
            return true;
        }
        map.insert(timestamp, true);
        if map.len() > 10000 {
            let keys: Vec<i64> = map.keys().take(5000).copied().collect();
            for key in keys {
                map.remove(&key);
            }
        }
        false
    }

    /// Sends a message to a number or group.
    pub async fn send_signal_message(
        &self,
        to: &str,
        content: &str,
        is_group: bool,
    ) -> Result<()> {
        let url = if is_group {
            format!(
                "{}/v2/send/{}",
                self.config.api_url, self.config.phone_number
            )
        } else {
            format!(
                "{}/v2/send/{}",
                self.config.api_url, self.config.phone_number
            )
        };

        let mut request = SignalSendRequest {
            message: content.to_string(),
            group: None,
        };

        if is_group {
            request.group = Some(to.to_string());
        }

        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "message": content,
                if is_group { "group" } else { "number" }: to,
            }))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("signal send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "signal send error: {body}"
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting Signal channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let http = self.http.clone();
        let receive_url = self.receive_url();
        let running = self.running.clone();
        let allow_from = self.config.allow_from.clone();

        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_secs(5);
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            loop {
                if !*running.read() {
                    break;
                }

                tokio::time::sleep(poll_interval).await;

                if !*running.read() {
                    break;
                }

                let resp = match http.get(&receive_url).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Signal poll error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    warn!("Signal poll returned {}", resp.status());
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }

                backoff = std::time::Duration::from_secs(1);

                let envelopes: Vec<SignalEnvelope> = match resp.json().await {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                for envelope in envelopes {
                    let inner = match envelope.envelope {
                        Some(e) => e,
                        None => continue,
                    };

                    let sender = inner.source_number.as_deref().unwrap_or("unknown");

                    // Filter by allow_from
                    if !allow_from.is_empty() && !allow_from.iter().any(|a| a == sender) {
                        continue;
                    }

                    let data_msg = match inner.data_message.as_ref() {
                        Some(m) => m,
                        None => continue,
                    };

                    let content = match data_msg.message.as_deref() {
                        Some(t) if !t.is_empty() => t,
                        _ => continue,
                    };

                    let timestamp = data_msg.timestamp.unwrap_or(0);

                    let chat_id = if let Some(ref group_info) = data_msg.group_info {
                        group_info.group_id.as_deref().unwrap_or(sender).to_string()
                    } else {
                        sender.to_string()
                    };

                    let inbound = InboundMessage {
                        channel: "signal".to_string(),
                        sender_id: sender.to_string(),
                        chat_id: chat_id.clone(),
                        content: content.to_string(),
                        media: Vec::new(),
                        session_key: chat_id,
                        correlation_id: String::new(),
                        metadata: {
                            let mut m = std::collections::HashMap::new();
                            m.insert("timestamp".to_string(), timestamp.to_string());
                            m
                        },
                    };

                    let _ = bus.send(inbound);
                }
            }

            info!("Signal receive loop stopped");
        });

        info!("Signal channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Signal channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "signal channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let is_group = msg.chat_id.len() > 20; // heuristic: group IDs are longer
        self.send_signal_message(&msg.chat_id, &msg.content, is_group)
            .await
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
    async fn test_signal_channel_new_validates() {
        let config = SignalConfig {
            api_url: String::new(),
            phone_number: String::new(),
            allow_from: Vec::new(),
        };
        assert!(SignalChannel::new(config, test_bus()).is_err());
    }

    #[tokio::test]
    async fn test_signal_channel_lifecycle() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".to_string(),
            phone_number: "+1234567890".to_string(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "signal");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_receive_url() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".to_string(),
            phone_number: "+1234567890".to_string(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();
        assert_eq!(
            ch.receive_url(),
            "http://localhost:8080/v1/receive/+1234567890"
        );
    }

    #[test]
    fn test_process_envelope_direct() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".to_string(),
            phone_number: "+1234567890".to_string(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();

        let envelope = SignalEnvelopeInner {
            source: Some("Alice".to_string()),
            source_number: Some("+9876543210".to_string()),
            source_uuid: None,
            timestamp: Some(1234567890),
            data_message: Some(SignalDataMessage {
                timestamp: Some(1234567890),
                message: Some("Hello".to_string()),
                group_info: None,
            }),
            sync_message: None,
        };

        let (sender, chat, content) = ch.process_envelope(&envelope).unwrap();
        assert_eq!(sender, "+9876543210");
        assert_eq!(chat, "+9876543210");
        assert_eq!(content, "Hello");
    }

    #[test]
    fn test_is_duplicate() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".to_string(),
            phone_number: "+1234567890".to_string(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();

        assert!(!ch.is_duplicate(100));
        assert!(ch.is_duplicate(100));
        assert!(!ch.is_duplicate(200));
    }

    // ---- New tests ----

    #[test]
    fn test_signal_config_fields() {
        let config = SignalConfig {
            api_url: "http://localhost:9090".into(),
            phone_number: "+1112223333".into(),
            allow_from: vec!["+999".into()],
        };
        assert_eq!(config.api_url, "http://localhost:9090");
        assert_eq!(config.phone_number, "+1112223333");
    }

    #[test]
    fn test_send_url() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".into(),
            phone_number: "+1234567890".into(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.send_url("+999"), "http://localhost:8080/v1/send/+999");
    }

    #[test]
    fn test_process_envelope_group_message() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".into(),
            phone_number: "+1234567890".into(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();

        let envelope = SignalEnvelopeInner {
            source: Some("Bob".into()),
            source_number: Some("+555".into()),
            source_uuid: None,
            timestamp: Some(999),
            data_message: Some(SignalDataMessage {
                timestamp: Some(999),
                message: Some("Group hello".into()),
                group_info: Some(SignalGroupInfo {
                    group_id: Some("group-1".into()),
                    name: Some("Test Group".into()),
                }),
            }),
            sync_message: None,
        };
        let (sender, chat, content) = ch.process_envelope(&envelope).unwrap();
        assert_eq!(sender, "+555");
        assert_eq!(chat, "group-1");
        assert_eq!(content, "Group hello");
    }

    #[test]
    fn test_process_envelope_no_data_message() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".into(),
            phone_number: "+1234567890".into(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();

        let envelope = SignalEnvelopeInner {
            source: None,
            source_number: None,
            source_uuid: None,
            timestamp: None,
            data_message: None,
            sync_message: None,
        };
        assert!(ch.process_envelope(&envelope).is_none());
    }

    #[test]
    fn test_is_duplicate_many() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".into(),
            phone_number: "+1234567890".into(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();

        for i in 0..100 {
            assert!(!ch.is_duplicate(i));
        }
        for i in 0..100 {
            assert!(ch.is_duplicate(i));
        }
    }

    #[tokio::test]
    async fn test_signal_double_stop() {
        let config = SignalConfig {
            api_url: "http://localhost:8080".into(),
            phone_number: "+1234567890".into(),
            allow_from: Vec::new(),
        };
        let ch = SignalChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();
        ch.stop().await.unwrap();
        ch.stop().await.unwrap();
    }
}

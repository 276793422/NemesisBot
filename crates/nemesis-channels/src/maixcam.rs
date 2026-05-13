//! MaixCam hardware channel (TCP server, JSON protocol).
//!
//! Receives detection events from MaixCam devices via TCP connections.
//! Supports person detection, heartbeat, and status update messages.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// MaixCam channel configuration.
#[derive(Debug, Clone)]
pub struct MaixCamConfig {
    /// Listen host.
    pub host: String,
    /// Listen port.
    pub port: u16,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

impl Default for MaixCamConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8888,
            allow_from: Vec::new(),
        }
    }
}

/// MaixCam message from device.
#[derive(Debug, Deserialize)]
pub struct MaixCamMessage {
    #[serde(rename = "type")]
    pub msg_type: Option<String>,
    pub tips: Option<String>,
    pub timestamp: Option<f64>,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

/// MaixCam command to device.
#[derive(Serialize)]
pub struct MaixCamCommand {
    #[serde(rename = "type")]
    pub cmd_type: String,
    pub timestamp: f64,
    pub message: String,
    pub chat_id: String,
}

/// MaixCam channel using TCP server.
pub struct MaixCamChannel {
    base: BaseChannel,
    config: MaixCamConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    client_count: Arc<parking_lot::RwLock<usize>>,
    outbound_queue: parking_lot::RwLock<Vec<OutboundMessage>>,
    /// Active TCP connections: chat_id -> write half.
    client_writers: dashmap::DashMap<String, tokio::io::WriteHalf<TcpStream>>,
    /// Cancellation sender for the accept loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl MaixCamChannel {
    /// Creates a new `MaixCamChannel`.
    pub fn new(config: MaixCamConfig) -> Result<Self> {
        Ok(Self {
            base: BaseChannel::new("maixcam"),
            config,
            running: Arc::new(parking_lot::RwLock::new(false)),
            client_count: Arc::new(parking_lot::RwLock::new(0)),
            outbound_queue: parking_lot::RwLock::new(Vec::new()),
            client_writers: dashmap::DashMap::new(),
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Returns the listen address.
    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.config.host, self.config.port)
    }

    /// Returns the number of connected devices.
    pub fn client_count(&self) -> usize {
        *self.client_count.read()
    }

    /// Increments client count.
    pub fn connect_client(&self) {
        *self.client_count.write() += 1;
    }

    /// Decrements client count.
    pub fn disconnect_client(&self) {
        let mut count = self.client_count.write();
        if *count > 0 {
            *count -= 1;
        }
    }

    /// Processes a MaixCam message.
    pub fn process_message(&self, msg: &MaixCamMessage) -> MaixCamEvent {
        let msg_type = msg.msg_type.as_deref().unwrap_or("");

        match msg_type {
            "person_detected" => {
                let data = msg.data.as_ref();
                let class_name = data
                    .and_then(|d| d.get("class_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("person");
                let score = data
                    .and_then(|d| d.get("score"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let x = data
                    .and_then(|d| d.get("x"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let y = data
                    .and_then(|d| d.get("y"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let w = data
                    .and_then(|d| d.get("w"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let h = data
                    .and_then(|d| d.get("h"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                let content = format!(
                    "Person detected!\nClass: {}\nConfidence: {:.2}%\nPosition: ({:.0}, {:.0})\nSize: {:.0}x{:.0}",
                    class_name, score * 100.0, x, y, w, h
                );

                let mut metadata = HashMap::new();
                if let Some(ts) = msg.timestamp {
                    metadata.insert("timestamp".to_string(), format!("{ts:.0}"));
                }
                metadata.insert("class_name".to_string(), class_name.to_string());
                metadata.insert("score".to_string(), format!("{score:.2}"));

                MaixCamEvent::PersonDetected {
                    content,
                    metadata,
                    sender_id: "maixcam".to_string(),
                    chat_id: "default".to_string(),
                }
            }
            "heartbeat" => MaixCamEvent::Heartbeat,
            "status" => {
                let data = msg.data.as_ref().map(|d| format!("{d:?}")).unwrap_or_default();
                MaixCamEvent::StatusUpdate(data)
            }
            _ => MaixCamEvent::Unknown(msg_type.to_string()),
        }
    }

    /// Builds a command to send to MaixCam devices.
    pub fn build_command(chat_id: &str, content: &str) -> MaixCamCommand {
        MaixCamCommand {
            cmd_type: "command".to_string(),
            timestamp: 0.0,
            message: content.to_string(),
            chat_id: chat_id.to_string(),
        }
    }

    /// Drains all queued outbound messages (for testing).
    pub fn drain_outbound(&self) -> Vec<OutboundMessage> {
        self.outbound_queue.write().drain(..).collect()
    }

    /// Spawns a TCP accept loop that listens for MaixCam device connections.
    fn spawn_tcp_server(&self) {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let running = self.running.clone();
        let client_count = self.client_count.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        tokio::spawn(async move {
            let listener = match TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!(addr = %addr, error = %e, "failed to bind MaixCam TCP listener");
                    return;
                }
            };

            info!(addr = %addr, "MaixCam TCP server listening");
            let mut cancel_rx = cancel_rx;

            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("MaixCam TCP server shutting down");
                        break;
                    }
                    result = listener.accept() => {
                        match result {
                            Ok((stream, peer)) => {
                                debug!(peer = %peer, "MaixCam device connected");
                                *client_count.write() += 1;

                                // Spawn per-connection reader
                                let running = running.clone();
                                let client_count = client_count.clone();
                                let peer_str = peer.to_string();

                                tokio::spawn(async move {
                                    let (reader, _) = stream.into_split();
                                    let mut reader = BufReader::new(reader);
                                    let mut line = String::new();

                                    loop {
                                        if !*running.read() {
                                            break;
                                        }
                                        line.clear();
                                        match reader.read_line(&mut line).await {
                                            Ok(0) => {
                                                info!(peer = %peer_str, "MaixCam device disconnected");
                                                break;
                                            }
                                            Ok(_) => {
                                                let trimmed = line.trim();
                                                if !trimmed.is_empty() {
                                                    debug!(peer = %peer_str, data = %trimmed, "MaixCam data");
                                                    // Parse JSON message
                                                    if let Ok(msg) = serde_json::from_str::<MaixCamMessage>(trimmed) {
                                                        let msg_type = msg.msg_type.as_deref().unwrap_or("");
                                                        debug!(msg_type = %msg_type, "MaixCam event");
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!(error = %e, "MaixCam read error");
                                                break;
                                            }
                                        }
                                    }

                                    *client_count.write() -= 1;
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "MaixCam accept error");
                            }
                        }
                    }
                }
            }
        });
    }
}

/// MaixCam event types.
#[derive(Debug)]
pub enum MaixCamEvent {
    PersonDetected {
        content: String,
        metadata: HashMap<String, String>,
        sender_id: String,
        chat_id: String,
    },
    Heartbeat,
    StatusUpdate(String),
    Unknown(String),
}

#[async_trait]
impl Channel for MaixCamChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!(
            host = %self.config.host,
            port = self.config.port,
            "starting MaixCam channel"
        );
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Spawn the TCP server
        self.spawn_tcp_server();

        info!("MaixCam channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping MaixCam channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Cancel the accept loop
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        *self.client_count.write() = 0;
        self.client_writers.clear();
        self.outbound_queue.write().clear();
        info!("MaixCam channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "maixcam channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        // Try to send to a specific device or broadcast
        if !self.client_writers.is_empty() {
            let cmd = Self::build_command(&msg.chat_id, &msg.content);
            let json = serde_json::to_string(&cmd)
                .map_err(|e| NemesisError::Channel(format!("MaixCam serialize error: {e}")))?;

            if let Some(mut writer) = self.client_writers.get_mut(&msg.chat_id) {
                writer
                    .write_all(format!("{json}\n").as_bytes())
                    .await
                    .map_err(|e| NemesisError::Channel(format!("MaixCam write error: {e}")))?;
                return Ok(());
            }

            // Broadcast to all
            for mut entry in self.client_writers.iter_mut() {
                let _ = entry
                    .write_all(format!("{json}\n").as_bytes())
                    .await;
            }
            return Ok(());
        }

        if *self.client_count.read() == 0 {
            return Err(NemesisError::Channel(
                "no connected MaixCam devices".to_string(),
            ));
        }

        debug!(chat_id = %msg.chat_id, "MaixCam sending command");
        self.outbound_queue.write().push(msg);
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
    async fn test_maixcam_channel_lifecycle() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        assert_eq!(ch.name(), "maixcam");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_listen_addr() {
        let config = MaixCamConfig {
            host: "0.0.0.0".to_string(),
            port: 9999,
            allow_from: Vec::new(),
        };
        let ch = MaixCamChannel::new(config).unwrap();
        assert_eq!(ch.listen_addr(), "0.0.0.0:9999");
    }

    #[test]
    fn test_process_person_detected() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let mut data = HashMap::new();
        data.insert("class_name".to_string(), serde_json::json!("person"));
        data.insert("score".to_string(), serde_json::json!(0.95));
        data.insert("x".to_string(), serde_json::json!(100.0));
        data.insert("y".to_string(), serde_json::json!(200.0));
        data.insert("w".to_string(), serde_json::json!(50.0));
        data.insert("h".to_string(), serde_json::json!(80.0));

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: None,
            timestamp: Some(1234567890.0),
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, metadata, .. } => {
                assert!(content.contains("Person detected"));
                assert!(content.contains("95.00%"));
                assert_eq!(metadata.get("class_name").unwrap(), "person");
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[test]
    fn test_process_heartbeat() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: Some("heartbeat".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        assert!(matches!(event, MaixCamEvent::Heartbeat));
    }

    #[test]
    fn test_build_command() {
        let cmd = MaixCamChannel::build_command("default", "take photo");
        assert_eq!(cmd.cmd_type, "command");
        assert_eq!(cmd.message, "take photo");
        assert_eq!(cmd.chat_id, "default");
    }

    #[tokio::test]
    async fn test_send_no_clients() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "default".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        assert!(ch.send(msg).await.is_err());
    }

    #[tokio::test]
    async fn test_send_with_clients() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        ch.connect_client();

        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "default".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let outbound = ch.drain_outbound();
        assert_eq!(outbound.len(), 1);
    }

    #[test]
    fn test_deserialize_message() {
        let json = r#"{"type":"person_detected","timestamp":1234.5,"data":{"class_name":"person","score":0.9}}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type.as_deref(), Some("person_detected"));
        assert_eq!(msg.timestamp.unwrap(), 1234.5);
    }

    // -- Additional tests --

    #[test]
    fn test_maixcam_config_default() {
        let config = MaixCamConfig::default();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8888);
        assert!(config.allow_from.is_empty());
    }

    #[test]
    fn test_maixcam_config_custom() {
        let config = MaixCamConfig {
            host: "192.168.1.1".into(),
            port: 9999,
            allow_from: vec!["device-1".into()],
        };
        assert_eq!(config.host, "192.168.1.1");
        assert_eq!(config.port, 9999);
        assert_eq!(config.allow_from.len(), 1);
    }

    #[test]
    fn test_build_command_fields() {
        let cmd = MaixCamChannel::build_command("chat-1", "take photo");
        assert_eq!(cmd.cmd_type, "command");
        assert_eq!(cmd.timestamp, 0.0);
        assert_eq!(cmd.message, "take photo");
        assert_eq!(cmd.chat_id, "chat-1");
    }

    #[test]
    fn test_build_command_serialization() {
        let cmd = MaixCamChannel::build_command("chat-1", "hello");
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"command\""));
        assert!(json.contains("\"message\":\"hello\""));
        assert!(json.contains("\"chat_id\":\"chat-1\""));
    }

    #[test]
    fn test_process_status_message() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let mut data = HashMap::new();
        data.insert("cpu".to_string(), serde_json::json!(45.2));
        data.insert("mem".to_string(), serde_json::json!(1024));

        let msg = MaixCamMessage {
            msg_type: Some("status".to_string()),
            tips: None,
            timestamp: Some(1234567890.0),
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::StatusUpdate(info) => {
                assert!(!info.is_empty());
            }
            _ => panic!("expected StatusUpdate event"),
        }
    }

    #[test]
    fn test_process_status_message_no_data() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: Some("status".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::StatusUpdate(info) => {
                assert!(info.is_empty() || info.contains("None"));
            }
            _ => panic!("expected StatusUpdate event"),
        }
    }

    #[test]
    fn test_process_unknown_message_type() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: Some("custom_event".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::Unknown(name) => {
                assert_eq!(name, "custom_event");
            }
            _ => panic!("expected Unknown event"),
        }
    }

    #[test]
    fn test_process_message_no_type() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: None,
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::Unknown(name) => {
                assert_eq!(name, "");
            }
            _ => panic!("expected Unknown event with empty name"),
        }
    }

    #[test]
    fn test_client_count_tracking() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        assert_eq!(ch.client_count(), 0);

        ch.connect_client();
        assert_eq!(ch.client_count(), 1);

        ch.connect_client();
        assert_eq!(ch.client_count(), 2);

        ch.disconnect_client();
        assert_eq!(ch.client_count(), 1);

        // Disconnect below zero should not go negative
        ch.disconnect_client();
        ch.disconnect_client();
        assert_eq!(ch.client_count(), 0);
    }

    #[test]
    fn test_person_detected_without_data() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, metadata, sender_id, chat_id } => {
                assert!(content.contains("Person detected"));
                assert!(content.contains("person")); // default class_name
                assert_eq!(sender_id, "maixcam");
                assert_eq!(chat_id, "default");
                // No timestamp in metadata when timestamp is None
                assert!(metadata.get("timestamp").is_none());
                assert_eq!(metadata.get("class_name").unwrap(), "person");
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[test]
    fn test_deserialize_message_minimal() {
        let json = r#"{}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert!(msg.msg_type.is_none());
        assert!(msg.tips.is_none());
        assert!(msg.timestamp.is_none());
        assert!(msg.data.is_none());
    }

    #[test]
    fn test_deserialize_message_with_tips() {
        let json = r#"{"type":"heartbeat","tips":"system ok","timestamp":999.0}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type.as_deref(), Some("heartbeat"));
        assert_eq!(msg.tips.as_deref(), Some("system ok"));
    }

    #[test]
    fn test_drain_outbound_empty() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        let outbound = ch.drain_outbound();
        assert!(outbound.is_empty());
    }

    // ---- Additional coverage tests ----

    #[tokio::test]
    async fn test_send_not_running() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        // Not started
        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "default".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not running"));
    }

    #[tokio::test]
    async fn test_stop_clears_state() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        ch.connect_client();
        assert_eq!(ch.client_count(), 1);

        ch.stop().await.unwrap();
        assert_eq!(ch.client_count(), 0);
        assert!(ch.drain_outbound().is_empty());
    }

    #[test]
    fn test_process_message_with_tips() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let mut data = HashMap::new();
        data.insert("class_name".to_string(), serde_json::json!("person"));
        data.insert("score".to_string(), serde_json::json!(0.8));

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: Some("Detection alert".to_string()),
            timestamp: Some(1234567890.0),
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, .. } => {
                assert!(content.contains("Person detected"));
                assert!(content.contains("80.00%"));
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[test]
    fn test_process_message_with_coordinates() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let mut data = HashMap::new();
        data.insert("class_name".to_string(), serde_json::json!("person"));
        data.insert("score".to_string(), serde_json::json!(0.75));
        data.insert("x".to_string(), serde_json::json!(10.0));
        data.insert("y".to_string(), serde_json::json!(20.0));
        data.insert("w".to_string(), serde_json::json!(100.0));
        data.insert("h".to_string(), serde_json::json!(200.0));

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: None,
            timestamp: Some(999.0),
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, metadata, .. } => {
                assert!(content.contains("Person detected"));
                assert!(content.contains("75.00%"));
                assert!(metadata.contains_key("score"));
                assert!(metadata.contains_key("class_name"));
                assert!(metadata.contains_key("timestamp"));
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[tokio::test]
    async fn test_start_stop_multiple_cycles() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        for _ in 0..3 {
            ch.start().await.unwrap();
            assert!(*ch.running.read());
            ch.stop().await.unwrap();
            assert!(!*ch.running.read());
        }
    }

    #[test]
    fn test_deserialize_status_with_data() {
        let json = r#"{"type":"status","timestamp":1234.5,"data":{"cpu":50.0,"mem":2048}}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type.as_deref(), Some("status"));
        assert!(msg.data.is_some());
    }

    #[tokio::test]
    async fn test_send_queues_when_no_writers_but_has_count() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        // Manually set client count but don't add writers
        *ch.client_count.write() = 1;

        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "default".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        // Should queue message since no writers match but count > 0
        ch.send(msg).await.unwrap();
        let outbound = ch.drain_outbound();
        assert_eq!(outbound.len(), 1);
    }

    #[test]
    fn test_build_command_serialization_roundtrip() {
        let cmd = MaixCamChannel::build_command("chat-1", "hello world");
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "command");
        assert_eq!(parsed["message"], "hello world");
        assert_eq!(parsed["chat_id"], "chat-1");
    }

    // --- Additional coverage tests ---

    #[tokio::test]
    async fn test_send_when_not_started() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        // Not started
        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "default".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not running"));
    }

    #[test]
    fn test_disconnect_client_decrements() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.connect_client();
        ch.connect_client();
        assert_eq!(ch.client_count(), 2);
        ch.disconnect_client();
        assert_eq!(ch.client_count(), 1);
        ch.disconnect_client();
        assert_eq!(ch.client_count(), 0);
    }

    #[test]
    fn test_disconnect_client_never_goes_negative() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        // Disconnect without connect
        ch.disconnect_client();
        assert_eq!(ch.client_count(), 0);
    }

    #[test]
    fn test_drain_outbound_when_empty() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        let drained = ch.drain_outbound();
        assert!(drained.is_empty());
    }

    #[test]
    fn test_drain_outbound_multiple() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.outbound_queue.write().push(OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "c1".to_string(),
            content: "msg1".to_string(),
            message_type: String::new(),
        });
        ch.outbound_queue.write().push(OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "c2".to_string(),
            content: "msg2".to_string(),
            message_type: String::new(),
        });
        let drained = ch.drain_outbound();
        assert_eq!(drained.len(), 2);
        // Queue should be empty after drain
        assert!(ch.drain_outbound().is_empty());
    }

    #[test]
    fn test_process_person_detected_with_timestamp() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let mut data = HashMap::new();
        data.insert("class_name".to_string(), serde_json::json!("vehicle"));
        data.insert("score".to_string(), serde_json::json!(0.75));
        data.insert("x".to_string(), serde_json::json!(10.0));
        data.insert("y".to_string(), serde_json::json!(20.0));
        data.insert("w".to_string(), serde_json::json!(30.0));
        data.insert("h".to_string(), serde_json::json!(40.0));

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: Some("alert".to_string()),
            timestamp: Some(1700000000.0),
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, metadata, sender_id, chat_id } => {
                assert!(content.contains("vehicle"));
                assert!(content.contains("75.00%"));
                assert!(metadata.contains_key("timestamp"));
                assert!(metadata.contains_key("score"));
                assert_eq!(sender_id, "maixcam");
                assert_eq!(chat_id, "default");
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[test]
    fn test_process_person_detected_defaults() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, metadata, .. } => {
                // Defaults: class_name="person", score=0.0, x/y/w/h=0.0
                assert!(content.contains("person"));
                assert!(content.contains("0.00%"));
                assert!(!metadata.contains_key("timestamp"));
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[test]
    fn test_deserialize_message_with_tips_field() {
        let json = r#"{"type":"person_detected","tips":"high confidence","timestamp":1234.5,"data":{}}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.tips.as_deref(), Some("high confidence"));
    }

    #[test]
    fn test_deserialize_minimal_message() {
        let json = r#"{}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert!(msg.msg_type.is_none());
        assert!(msg.tips.is_none());
        assert!(msg.timestamp.is_none());
        assert!(msg.data.is_none());
    }

    #[tokio::test]
    async fn test_stop_clears_writers() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        assert!(*ch.running.read());
        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
        assert!(ch.client_writers.is_empty());
        assert_eq!(*ch.client_count.read(), 0);
    }

    #[test]
    fn test_listen_addr_custom() {
        let config = MaixCamConfig {
            host: "127.0.0.1".into(),
            port: 7777,
            allow_from: Vec::new(),
        };
        let ch = MaixCamChannel::new(config).unwrap();
        assert_eq!(ch.listen_addr(), "127.0.0.1:7777");
    }

    #[tokio::test]
    async fn test_send_no_clients_returns_error() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        // client_count = 0, client_writers empty -> should error
        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "default".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no connected") || err.contains("not running"));
    }

    #[test]
    fn test_process_message_person_detected_no_data() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };
        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, .. } => {
                assert!(content.contains("Person detected"));
                assert!(content.contains("0.00%"));
            }
            _ => panic!("expected PersonDetected event"),
        }
    }

    #[test]
    fn test_process_message_status_with_data() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        let mut data = HashMap::new();
        data.insert("cpu".to_string(), serde_json::json!(80.0));
        let msg = MaixCamMessage {
            msg_type: Some("status".to_string()),
            tips: None,
            timestamp: Some(1234.0),
            data: Some(data),
        };
        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::StatusUpdate(data_str) => {
                assert!(data_str.contains("cpu"));
            }
            _ => panic!("expected StatusUpdate event"),
        }
    }

    #[test]
    fn test_process_message_unknown_type() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        let msg = MaixCamMessage {
            msg_type: Some("custom_event".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };
        let event = ch.process_message(&msg);
        assert!(matches!(event, MaixCamEvent::Unknown(ref s) if s == "custom_event"));
    }

    #[test]
    fn test_process_message_empty_type() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        let msg = MaixCamMessage {
            msg_type: None,
            tips: None,
            timestamp: None,
            data: None,
        };
        let event = ch.process_message(&msg);
        assert!(matches!(event, MaixCamEvent::Unknown(ref s) if s.is_empty()));
    }

    #[test]
    fn test_build_command_timestamp_zero() {
        let cmd = MaixCamChannel::build_command("test-chat", "test msg");
        assert_eq!(cmd.timestamp, 0.0);
        assert_eq!(cmd.cmd_type, "command");
        assert_eq!(cmd.chat_id, "test-chat");
        assert_eq!(cmd.message, "test msg");
    }

    #[test]
    fn test_deserialize_maixcam_message_with_all_fields() {
        let json = r#"{"type":"person_detected","tips":"Alert!","timestamp":123456.789,"data":{"class_name":"cat","score":0.92}}"#;
        let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type.as_deref(), Some("person_detected"));
        assert_eq!(msg.tips.as_deref(), Some("Alert!"));
        assert_eq!(msg.timestamp.unwrap(), 123456.789);
        assert!(msg.data.is_some());
        let data = msg.data.unwrap();
        assert_eq!(data.get("class_name").unwrap().as_str(), Some("cat"));
    }

    #[test]
    fn test_maixcam_command_serialize() {
        let cmd = MaixCamChannel::build_command("c1", "hello");
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"command\""));
        assert!(json.contains("\"chat_id\":\"c1\""));
        assert!(json.contains("\"message\":\"hello\""));
    }

    #[tokio::test]
    async fn test_start_stop_clears_writers_and_queue() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        ch.connect_client();
        ch.outbound_queue.write().push(OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "c1".to_string(),
            content: "msg".to_string(),
            message_type: String::new(),
        });
        assert_eq!(ch.client_count(), 1);
        assert_eq!(ch.outbound_queue.read().len(), 1);

        ch.stop().await.unwrap();
        assert_eq!(ch.client_count(), 0);
        assert!(ch.outbound_queue.read().is_empty());
        assert!(ch.client_writers.is_empty());
    }

    #[test]
    fn test_maixcam_event_debug_format() {
        let event = MaixCamEvent::Heartbeat;
        let debug = format!("{:?}", event);
        assert!(debug.contains("Heartbeat"));

        let event = MaixCamEvent::Unknown("test".to_string());
        let debug = format!("{:?}", event);
        assert!(debug.contains("test"));

        let event = MaixCamEvent::StatusUpdate("cpu=80".to_string());
        let debug = format!("{:?}", event);
        assert!(debug.contains("cpu=80"));
    }

    // ============================================================
    // Additional coverage tests for 95%+ target (round 2)
    // ============================================================

    #[test]
    fn test_maixcam_event_person_detected_debug() {
        let event = MaixCamEvent::PersonDetected {
            content: "Test".to_string(),
            metadata: HashMap::new(),
            sender_id: "cam1".to_string(),
            chat_id: "chat1".to_string(),
        };
        let debug = format!("{:?}", event);
        assert!(debug.contains("PersonDetected"));
    }

    #[test]
    fn test_person_detected_partial_data() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        // Only class_name, no coordinates
        let mut data = HashMap::new();
        data.insert("class_name".to_string(), serde_json::json!("cat"));
        data.insert("score".to_string(), serde_json::json!(0.5));

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: None,
            timestamp: None,
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, .. } => {
                assert!(content.contains("cat"));
                assert!(content.contains("50.00%"));
                // x/y/w/h default to 0.0
                assert!(content.contains("0"));
            }
            _ => panic!("expected PersonDetected"),
        }
    }

    #[test]
    fn test_person_detected_with_non_standard_class() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let mut data = HashMap::new();
        data.insert("class_name".to_string(), serde_json::json!("vehicle"));
        data.insert("score".to_string(), serde_json::json!(1.0));

        let msg = MaixCamMessage {
            msg_type: Some("person_detected".to_string()),
            tips: Some("High confidence detection".to_string()),
            timestamp: Some(9999.0),
            data: Some(data),
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::PersonDetected { content, metadata, .. } => {
                assert!(content.contains("vehicle"));
                assert!(content.contains("100.00%"));
                assert_eq!(metadata.get("class_name").unwrap(), "vehicle");
                assert!(metadata.contains_key("timestamp"));
            }
            _ => panic!("expected PersonDetected"),
        }
    }

    #[test]
    fn test_status_with_no_data() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();

        let msg = MaixCamMessage {
            msg_type: Some("status".to_string()),
            tips: None,
            timestamp: None,
            data: None,
        };

        let event = ch.process_message(&msg);
        match event {
            MaixCamEvent::StatusUpdate(data) => {
                assert!(data.is_empty() || data.contains("None"));
            }
            _ => panic!("expected StatusUpdate"),
        }
    }

    #[tokio::test]
    async fn test_send_with_queued_messages_and_no_writers() {
        let config = MaixCamConfig::default();
        let ch = MaixCamChannel::new(config).unwrap();
        ch.start().await.unwrap();
        // Set client count > 0 but no writers
        *ch.client_count.write() = 2;

        let msg = OutboundMessage {
            channel: "maixcam".to_string(),
            chat_id: "device-1".to_string(),
            content: "command".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let drained = ch.drain_outbound();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].chat_id, "device-1");
        assert_eq!(drained[0].content, "command");
    }

    #[test]
    fn test_build_command_empty_message() {
        let cmd = MaixCamChannel::build_command("chat-1", "");
        assert_eq!(cmd.message, "");
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"message\":\"\""));
    }
}

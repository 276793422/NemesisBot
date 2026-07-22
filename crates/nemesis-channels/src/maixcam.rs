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
                    class_name,
                    score * 100.0,
                    x,
                    y,
                    w,
                    h
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
                let data = msg
                    .data
                    .as_ref()
                    .map(|d| format!("{d:?}"))
                    .unwrap_or_default();
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
                    error!(addr = %addr, error = %e, "[MaixCamChannel] failed to bind MaixCam TCP listener");
                    return;
                }
            };

            info!(addr = %addr, "[MaixCamChannel] TCP server listening");
            let mut cancel_rx = cancel_rx;

            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("[MaixCamChannel] TCP server shutting down");
                        break;
                    }
                    result = listener.accept() => {
                        match result {
                            Ok((stream, peer)) => {
                                debug!(peer = %peer, "[MaixCamChannel] device connected");
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
                                                info!(peer = %peer_str, "[MaixCamChannel] device disconnected");
                                                break;
                                            }
                                            Ok(_) => {
                                                let trimmed = line.trim();
                                                if !trimmed.is_empty() {
                                                    debug!(peer = %peer_str, data = %trimmed, "[MaixCamChannel] data");
                                                    // Parse JSON message
                                                    if let Ok(msg) = serde_json::from_str::<MaixCamMessage>(trimmed) {
                                                        let msg_type = msg.msg_type.as_deref().unwrap_or("");
                                                        debug!(msg_type = %msg_type, "[MaixCamChannel] event");
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!(error = %e, "[MaixCamChannel] read error");
                                                break;
                                            }
                                        }
                                    }

                                    *client_count.write() -= 1;
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "[MaixCamChannel] accept error");
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

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!(
            host = %self.config.host,
            port = self.config.port,
            "[MaixCamChannel] starting MaixCam channel"
        );
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Spawn the TCP server
        self.spawn_tcp_server();

        info!("[MaixCamChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[MaixCamChannel] stopping MaixCam channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Cancel the accept loop
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        *self.client_count.write() = 0;
        self.client_writers.clear();
        self.outbound_queue.write().clear();
        info!("[MaixCamChannel] channel stopped");
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
                let _ = entry.write_all(format!("{json}\n").as_bytes()).await;
            }
            return Ok(());
        }

        if *self.client_count.read() == 0 {
            return Err(NemesisError::Channel(
                "no connected MaixCam devices".to_string(),
            ));
        }

        debug!(chat_id = %msg.chat_id, "[MaixCamChannel] sending command");
        self.outbound_queue.write().push(msg);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

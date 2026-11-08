//! MaixCam hardware channel (TCP server, JSON protocol).
//!
//! Receives detection events from MaixCam devices via TCP connections.
//! Supports person detection, heartbeat, and status update messages.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
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
    /// Message bus sender for publishing inbound device events.
    bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    running: Arc<parking_lot::RwLock<bool>>,
    client_count: Arc<parking_lot::RwLock<usize>>,
    outbound_queue: parking_lot::RwLock<Vec<OutboundMessage>>,
    /// Active TCP connections: chat_id -> write half（into_split 的
    /// OwnedWriteHalf）。Arc<DashMap> 外层：spawn_tcp_server 需把同一张表
    /// move 进 reader 任务——DashMap 的 Clone 是**逐条目深拷贝**而非共享，
    /// 裸字段 clone 会让 reader 的 insert 落进副本、send 查的原始表永远
    /// 为空（出站管线仍旧死代码）。内层 Arc+tokio::Mutex 双包：send 出站
    /// 先把 Arc 克隆出 DashMap 锁、再 lock 拿 &mut 写（DashMap 分片锁不得
    /// 跨 await——写超时 30s 会把并发取锁方全部阻塞；tokio::Mutex 本身就
    /// 是为 guard 跨 await 设计的异步感知锁，写期间串行化同一 socket 的
    /// 并发写）。
    client_writers:
        Arc<dashmap::DashMap<String, Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>>>,
    /// Cancellation sender for the accept loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl MaixCamChannel {
    /// Creates a new `MaixCamChannel`.
    ///
    /// `bus_sender` publishes inbound device events (person detection) to the
    /// message bus — parity with Go's `NewMaixCamChannel(cfg, bus)` ->
    /// `HandleMessage`（修复前设备消息只 debug 日志直接丢弃）.
    pub fn new(
        config: MaixCamConfig,
        bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        Ok(Self {
            base: BaseChannel::with_allow_list("maixcam", config.allow_from.clone()),
            config,
            bus_sender,
            running: Arc::new(parking_lot::RwLock::new(false)),
            client_count: Arc::new(parking_lot::RwLock::new(0)),
            outbound_queue: parking_lot::RwLock::new(Vec::new()),
            client_writers: Arc::new(dashmap::DashMap::new()),
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
        dec_client_count(&self.client_count);
    }

    /// Processes a MaixCam message.
    pub fn process_message(&self, msg: &MaixCamMessage) -> MaixCamEvent {
        Self::classify_message(msg)
    }

    /// Classifies a raw device message into an event（统一定义，
    /// `process_message` 与 TCP reader 共用）.
    pub fn classify_message(msg: &MaixCamMessage) -> MaixCamEvent {
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
    ///
    /// bind 在 spawn **之前**完成并把失败传播给调用方（wechat/wecom 同款）——
    /// 此前 bind 失败只在后台任务里 error，start() 照常报成功，入站能力
    /// 归零而状态面显示运行中。
    async fn spawn_tcp_server(&self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let running = self.running.clone();
        let client_count = self.client_count.clone();
        let client_writers = self.client_writers.clone();
        let bus_sender = self.bus_sender.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
            NemesisError::Channel(format!(
                "[MaixCamChannel] failed to bind MaixCam TCP listener {addr}: {e}"
            ))
        })?;

        tokio::spawn(async move {
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
                            Ok((stream, peer)) => {                                debug!(peer = %peer, "[MaixCamChannel] device connected");
                                *client_count.write() += 1;

                                // Spawn per-connection reader
                                let running = running.clone();
                                let client_count = client_count.clone();
                                let client_writers = client_writers.clone();
                                let bus_sender = bus_sender.clone();
                                let peer_str = peer.to_string();

                                tokio::spawn(async move {
                                    // write half 注册进 client_writers（key 与
                                    // classify_message 硬编码 chat_id="default"、
                                    // send 按 msg.chat_id 查表三方同键——MaixCam
                                    // 协议无设备 id 握手，单设备假设；重连时
                                    // insert 覆盖旧 half 自愈，断开不 remove
                                    // 避免误删重连后的新注册，stop 统一 clear）。
                                    // 修复前 write half 被直接丢弃——出站定向/
                                    // 广播分支死代码，bot 回复永远到不了设备。
                                    let (reader, writer) = stream.into_split();
                                    client_writers.insert(
                                        "default".to_string(),
                                        Arc::new(tokio::sync::Mutex::new(writer)),
                                    );
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
                                                    // Parse JSON message and publish to the bus
                                                    // (修复前解析结果直接丢弃)
                                                    if let Ok(msg) = serde_json::from_str::<MaixCamMessage>(trimmed) {
                                                        let msg_type = msg.msg_type.as_deref().unwrap_or("");
                                                        debug!(msg_type = %msg_type, "[MaixCamChannel] event");
                                                        match MaixCamChannel::classify_message(&msg) {
                                                            MaixCamEvent::PersonDetected { content, metadata, sender_id, chat_id } => {
                                                                let inbound = InboundMessage {
                                                                    channel: "maixcam".to_string(),
                                                                    sender_id,
                                                                    chat_id,
                                                                    content,
                                                                    media: Vec::new(),
                                                                    session_key: "maixcam:default".to_string(),
                                                                    correlation_id: String::new(),
                                                                    metadata,
                                                                    voice_playback: None,
                                                                };
                                                                if let Err(e) = bus_sender.send(inbound) {
                                                                    warn!(error = %e, "[MaixCamChannel] failed to publish inbound message");
                                                                }
                                                            }
                                                            MaixCamEvent::Heartbeat => {
                                                                debug!(peer = %peer_str, "[MaixCamChannel] heartbeat");
                                                            }
                                                            MaixCamEvent::StatusUpdate(data) => {
                                                                info!(peer = %peer_str, status = %data, "[MaixCamChannel] status update");
                                                            }
                                                            MaixCamEvent::Unknown(t) => {
                                                                debug!(msg_type = %t, "[MaixCamChannel] unknown message type");
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!(error = %e, "[MaixCamChannel] read error");
                                                break;
                                            }
                                        }
                                    }

                                    // 计数下溢守卫：单一真相源 dec_client_count
                                    //（0 守卫，stop() 清零后 reader 退出不减到负）。
                                    dec_client_count(&client_count);
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
        Ok(())
    }
}

/// 连接计数递减（0 守卫防下溢）。`disconnect_client` 与 reader 退出路径
/// 共用——stop() 会强制清零（`*count = 0`），此时已连接的 reader 随
/// running=false 退出也会走这里：无守卫减法 0-1 在 debug 下 panic、
/// release 下 wrap 成 usize::MAX（状态面显示天文数字）。
fn dec_client_count(count: &parking_lot::RwLock<usize>) {
    let mut c = count.write();
    if *c > 0 {
        *c -= 1;
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

impl MaixCamChannel {
    /// 出站写超时：manager 出站派发是单任务串行
    /// await（一条通道 send 无界挂起 = 全部通道出站停摆且零错误零日志，
    /// 见 base.rs 串行派发纪律）——设备停摆（对端不读、内核缓冲填满）
    /// 时定向写/broadcast 写会把整条出站管线冻死，TCP write 必须有界。
    /// HTTP 35s / email 30s 同族先例，此处对齐 email 的 30s。
    const WRITE_TIMEOUT_SECS: u64 = 30;

    /// 单一写路径：带超时的 write_all（有界性）。
    /// 超时映射 `ErrorKind::TimedOut`（email io_timeout 同形态）。
    /// 泛型形态：生产传 TcpStream 写半部，测试传 mock writer（Windows
    /// loopback TCP 缓冲自调连显式收窄都不认——内核行为不可依赖）。
    async fn write_payload_with_timeout<W: tokio::io::AsyncWrite + Unpin>(
        writer: &mut W,
        payload: &str,
        timeout: std::time::Duration,
    ) -> std::io::Result<()> {
        match tokio::time::timeout(timeout, writer.write_all(payload.as_bytes())).await {
            Ok(r) => r,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("MaixCam write timed out after {}s", timeout.as_secs()),
            )),
        }
    }

    /// send 的可注入超时形态（测试用短窗验证 的
    /// 有界性；生产走 WRITE_TIMEOUT_SECS）。
    async fn send_with_timeout(
        &self,
        msg: OutboundMessage,
        timeout: std::time::Duration,
    ) -> Result<()> {
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
            let payload = format!("{json}\n");

            // 先把 Arc 克隆出 DashMap 锁、再 lock 拿 &mut 写（锁不跨 await）
            if let Some(writer) = self
                .client_writers
                .get_mut(&msg.chat_id)
                .map(|g| Arc::clone(g.value()))
            {
                let mut guard = writer.lock().await;
                Self::write_payload_with_timeout(&mut *guard, &payload, timeout)
                    .await
                    .map_err(|e| NemesisError::Channel(format!("MaixCam write error: {e}")))?;
                return Ok(());
            }

            // Broadcast to all（best-effort 语义保持：单设备失败不拦整体，
            // 但每设备写有界——不再被停摆设备冻死派发管线；快照收集在锁外写）
            let writers: Vec<_> = self
                .client_writers
                .iter()
                .map(|e| Arc::clone(e.value()))
                .collect();
            for writer in writers {
                let mut guard = writer.lock().await;
                let _ = Self::write_payload_with_timeout(&mut *guard, &payload, timeout).await;
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
        self.spawn_tcp_server().await?;

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
        Self::send_with_timeout(
            self,
            msg,
            std::time::Duration::from_secs(Self::WRITE_TIMEOUT_SECS),
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

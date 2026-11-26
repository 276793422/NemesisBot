//! Slack Socket Mode channel (events API, thread support).
//!
//! Uses Slack's Socket Mode for receiving events and the Web API for
//! sending messages. Supports thread replies and reaction-based
//! acknowledgment (checkmark on reply; the ack store is populated via
//! `store_pending_ack`, currently unwired — eyes-on-receive is not
//! implemented). Slash-command / interactive envelopes are acked
//! (protocol contract, BUG-20261121-052) but their payloads are not
//! processed; file download is not implemented (media is always empty).

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel, EventDedup};

const SLACK_API_BASE: &str = "https://slack.com/api";
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

/// Socket Mode 写侧句柄类型（`handle_envelope` 统一 ack 的落点；测试用
/// 同型 client split sink 直接驱动该单元）。
type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;

// ---------------------------------------------------------------------------
// Slack API types
// ---------------------------------------------------------------------------

/// Slack channel configuration.
/// SEC-002（BUG-20261111-068）：双 token 脱敏。
#[derive(Clone)]
pub struct SlackConfig {
    /// Bot token (xoxb-...).
    pub bot_token: String,
    /// App-level token (xapp-...).
    pub app_token: String,
    /// Allowed user IDs (empty = allow all).
    pub allow_from: Vec<String>,
}

crate::debug_redact::impl_redacted_debug!(SlackConfig {
    plain { allow_from }
    secret { bot_token, app_token }
});

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
    /// 事件幂等集（BUG-20261121-051：同 ts 双投递/平台重推收敛）。
    event_dedup: Arc<EventDedup>,
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
            http: crate::base::outbound_http_client()?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            bot_user_id: Arc::new(parking_lot::RwLock::new(String::new())),
            pending_acks: dashmap::DashMap::new(),
            transcriber: parking_lot::RwLock::new(None),
            event_dedup: Arc::new(EventDedup::new()),
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
        text.replace(&format!("<@{bot_id}>"), "").trim().to_string()
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
            return Err(NemesisError::Channel(format!(
                "slack auth.test failed: {err}"
            )));
        }

        let user_id = resp["user_id"].as_str().unwrap_or("unknown").to_string();
        Ok(user_id)
    }

    /// Gets a Socket Mode WebSocket URL from Slack.
    async fn get_socket_mode_url(&self) -> Result<String> {
        let resp: serde_json::Value = self
            .http
            .post(format!("{SLACK_API_BASE}/apps.connections.open"))
            .header("Authorization", format!("Bearer {}", self.config.app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("slack apps.connections.open failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                NemesisError::Channel(format!("slack apps.connections.open parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(NemesisError::Channel(format!(
                "slack apps.connections.open failed: {err}"
            )));
        }

        resp["url"].as_str().map(String::from).ok_or_else(|| {
            NemesisError::Channel("missing 'url' in connections.open response".to_string())
        })
    }

    /// Starts the Socket Mode WebSocket receive loop.
    fn start_socket_mode_loop(&self) {
        let http = self.http.clone();
        let app_token = self.config.app_token.clone();
        let bot_user_id = self.bot_user_id.clone();
        let allow_from = self.config.allow_from.clone();
        let running = self.running.clone();
        let event_dedup = self.event_dedup.clone();
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
                                warn!(
                                    "[SlackChannel] failed to parse connections.open response: {e}"
                                );
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(MAX_BACKOFF);
                                continue;
                            }
                        },
                        Err(e) => {
                            warn!(
                                "[SlackChannel] failed to get WebSocket URL: {e}, retrying in {backoff:?}"
                            );
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    };

                    if body["ok"].as_bool() != Some(true) {
                        let err = body["error"].as_str().unwrap_or("unknown");
                        warn!(
                            "[SlackChannel] connections.open error: {err}, retrying in {backoff:?}"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }

                    match body["url"].as_str() {
                        Some(u) => u.to_string(),
                        None => {
                            warn!("[SlackChannel] missing URL in connections.open response");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                };

                info!("[SlackChannel] connecting to Socket Mode...");

                let ws_result = crate::base::ws_connect_bounded(&ws_url).await;
                let ws_stream = match ws_result {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        warn!(
                            "[SlackChannel] WebSocket connection failed: {e}, retrying in {backoff:?}"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                backoff = INITIAL_BACKOFF;
                info!("[SlackChannel] Socket Mode connected");

                let (mut ws_tx, mut ws_rx) = ws_stream.split();

                let should_reconnect = 'inner: loop {
                    // BUG-20261127-018：stop() 即时生效——健康长连接上
                    // ws_rx.next() 可无限挂起，纯旗标检查只在重连决策点
                    // （should_reconnect break 后），stop() 置位后旧会话
                    // 继续收发直到连接自然断开（禁用通道后入站消息继续
                    // 喂 agent）。200ms 粒度轮询家族原语（nostr/mattermost
                    // 同款语义，base.rs 单一实现）。
                    let msg = match crate::base::ws_message_or_stop(&mut ws_rx, &running).await {
                        // stop() 置位：不重连，外层 `!should_reconnect ||
                        // !*running` 即退（重连决策点不再执行）。
                        // 流结束/错误是 Item(None/Err)，照旧走下方重连臂。
                        crate::base::WsWait::Stopped => break 'inner false,
                        crate::base::WsWait::Item(item) => item,
                    };
                    // 解包 Option<Result<..>> → Message（原第一级 match 臂不变）
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            warn!("[SlackChannel] WebSocket error: {e}");
                            break 'inner true;
                        }
                        None => {
                            info!("[SlackChannel] WebSocket closed");
                            break 'inner true;
                        }
                    };
                    let text = match msg {
                        tokio_tungstenite::tungstenite::Message::Text(t) => t,
                        tokio_tungstenite::tungstenite::Message::Close(_) => {
                            info!("[SlackChannel] Socket Mode closed by server");
                            break 'inner true;
                        }
                        _ => continue,
                    };

                    // BUG-20261121-052：信封处理提取为可测单元——统一 ack
                    // + events_api 解析发布（含 BUG-20261121-051 ts 幂等）。
                    if Self::handle_envelope(
                        &text,
                        &mut ws_tx,
                        &bot_user_id,
                        &allow_from,
                        &event_dedup,
                        &bus_sender,
                    )
                    .await
                    {
                        break 'inner true;
                    }
                };

                if !should_reconnect || !*running.read() {
                    break;
                }

                warn!("[SlackChannel] reconnecting in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("[SlackChannel] Socket Mode loop stopped");
        });
    }

    /// 处理单个 Socket Mode 信封（BUG-20261121-052 重构为可测单元）。
    ///
    /// 契约（官方 Socket Mode 协议）：**所有携带 `envelope_id` 的信封必须
    /// 在 3 秒内回 ack**（`{"envelope_id": ...}`），未 ack 的信封 Slack 必
    /// 重推（重推风暴）。hello/disconnect 不带 envelope_id 天然豁免。旧实
    /// 现只在 events_api 臂 ack——slash_commands / interactive 等信封从不
    /// ack（模块头宣称支持 slash commands 而实际连 ack 契约都不满足）。
    /// 修复后 ack 统一在信封层，事件解析发布在后（仅 events_api 臂）。
    ///
    /// 返回 true = 应重连（disconnect 信封 / ack 写失败）。
    async fn handle_envelope(
        text: &str,
        ws_tx: &mut WsSink,
        bot_user_id: &Arc<parking_lot::RwLock<String>>,
        allow_from: &[String],
        event_dedup: &EventDedup,
        bus_sender: &broadcast::Sender<InboundMessage>,
    ) -> bool {
        let payload: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warn!("[SlackChannel] failed to parse message: {e}");
                return false;
            }
        };

        let envelope_type = payload["type"].as_str().unwrap_or("");

        // 统一信封 ack（BUG-20261121-052）
        let envelope_id = payload["envelope_id"].as_str().unwrap_or("");
        if !envelope_id.is_empty() {
            let ack = serde_json::json!({ "envelope_id": envelope_id });
            if let Err(e) = ws_tx
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&ack).unwrap().into(),
                ))
                .await
            {
                error!("[SlackChannel] failed to send ack: {e}");
                return true;
            }
        }

        match envelope_type {
            "hello" => {
                debug!("[SlackChannel] Socket Mode hello received");
            }

            "events_api" => {
                // Extract the event
                let event = &payload["payload"]["event"];
                if let Some(inbound) =
                    Self::parse_slack_event(event, bot_user_id, allow_from, event_dedup)
                {
                    debug!(
                        "[SlackChannel] message from {} in channel {}",
                        inbound.sender_id, inbound.chat_id
                    );
                    if bus_sender.send(inbound).is_err() {
                        warn!("[SlackChannel] failed to publish inbound message (no receivers)");
                    }
                }
            }

            "disconnect" => {
                let reason = payload["reason"].as_str().unwrap_or("unknown");
                info!("[SlackChannel] disconnect request: {reason}");
                return true;
            }

            _ => {
                debug!("[SlackChannel] envelope type: {envelope_type}");
            }
        }

        false
    }

    /// Parses a Slack event into an InboundMessage.
    fn parse_slack_event(
        event: &serde_json::Value,
        bot_user_id: &Arc<parking_lot::RwLock<String>>,
        allow_from: &[String],
        event_dedup: &EventDedup,
    ) -> Option<InboundMessage> {
        let event_type = event["type"].as_str()?;
        if event_type != "message" && event_type != "app_mention" {
            return None;
        }

        // BUG-20261121-051：非空 subtype 整类丢弃——message_changed（编辑
        // / 链接 unfurl 更新）重灌 = 同一条消息二次喂 agent（discord
        // BUG-20261121-042 同族裁决：编辑与 unfurl 都是重复形态，不重灌
        // ——同日 R2160 已定全局立场，本通道对齐）；message_deleted /
        // channel_join / message_replied 等其余 subtype 本就无 agent 消费
        // 语义。
        if event["subtype"].as_str().is_some() {
            return None;
        }

        // Filter out bot messages
        if event.get("bot_id").is_some() {
            return None;
        }

        let user_id = event["user"].as_str()?;

        // Filter out own messages
        let bot_id = bot_user_id.read().clone();
        if !bot_id.is_empty() && user_id == bot_id {
            return None;
        }

        // Filter by allow_from
        if !allow_from.is_empty() && !crate::base::allow_list_contains(allow_from, user_id) {
            debug!("[SlackChannel] ignoring message from unlisted user {user_id}");
            return None;
        }

        let channel = event["channel"].as_str()?;
        let text = event["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        let ts = event["ts"].as_str().unwrap_or("0");

        // BUG-20261121-051：ts 级幂等闸——同一条消息的 message 与
        // app_mention 双投递（双订阅结构性双事件，同 channel+ts）与平台
        // 重推（ack 未达窗口，R1316 遗留加固项）都在此收敛为一次，放行
        // 前最后一道闸（只对将要发布的消息记账）。ts 缺失（守恒默认
        // "0"）保守放行宁重复不丢（feishu BUG-048 同款取舍）。
        let dedup_key = format!("{channel}/{ts}");
        if ts != "0" && !event_dedup.check_and_record(&dedup_key) {
            debug!("[SlackChannel] duplicate event dropped (ts {ts} in {channel})");
            return None;
        }

        let thread_ts = event["thread_ts"].as_str().map(|s| s.to_string());

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

        Some(
            InboundMessage::user_ingress(
                "slack".to_string(),
                user_id.to_string(),
                chat_id,
                text.to_string(),
            )
            .with_metadata_map(metadata),
        )
    }

    /// Adds a reaction to a message.
    pub async fn add_reaction(&self, channel: &str, timestamp: &str, emoji: &str) -> Result<()> {
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

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        // BUG-20261121-050：幂等双启守卫 + base 旗标接线（telegram
        // BUG-20261110-037 / discord BUG-20261121-043 / feishu
        // BUG-20261121-046 同构家族第四批）——无守卫的重复 start 会
        // spawn 多个并行 Socket Mode loop 每条消息 N 倍喂 agent；trait
        // is_running()（读 base 旗标）恒 false 使 manager 运行中守卫失效。
        if self.base.is_running() {
            info!("[SlackChannel] already running, skipping duplicate start");
            return Ok(());
        }

        info!("[SlackChannel] starting (Socket Mode)");

        // 先置位再 spawn（BUG-20261121-050 附带顺序修复：旧顺序
        // start_socket_mode_loop 先于 running=true，spawn 的循环首行读
        // running=false 即自杀退出 = 通道假死；discord BUG-20261121-043
        // 同款顺序形态修复）。
        *self.running.write() = true;
        self.base.set_running(true);
        self.base.set_enabled(true);

        // Validate bot token
        match self.validate_bot_token().await {
            Ok(user_id) => {
                *self.bot_user_id.write() = user_id.clone();
                info!("[SlackChannel] bot authenticated (user_id: {user_id})");
            }
            Err(e) => {
                warn!("[SlackChannel] auth.test failed (continuing anyway): {e}");
            }
        }

        // Start Socket Mode loop
        self.start_socket_mode_loop();

        info!("[SlackChannel] started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[SlackChannel] stopping");
        *self.running.write() = false;
        // BUG-20261121-050：对称清 base 旗标（契约面见 start 内注释）。
        self.base.set_running(false);
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
            let _ = self
                .add_reaction(&msg_ref.channel_id, &msg_ref.timestamp, "white_check_mark")
                .await;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

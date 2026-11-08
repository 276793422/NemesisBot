//! DingTalk channel (Stream Mode, WebSocket, markdown/text messages).
//!
//! Uses DingTalk Stream Mode for receiving messages and the session webhook
//! for sending replies. The Stream Mode uses a long-lived HTTP connection
//! to receive events from the DingTalk server.

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
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

/// DingTalk channel configuration.
#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    /// Client ID from DingTalk Open Platform.
    pub client_id: String,
    /// Client Secret.
    pub client_secret: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// DingTalk bot callback data.
#[derive(Debug, Deserialize)]
pub struct DingTalkCallbackData {
    pub text: DingTalkTextContent,
    /// fix：与 DingTalkConversationMessage 同类，真实 wire 是 camelCase。
    #[serde(rename = "senderStaffId", alias = "sender_staff_id")]
    pub sender_staff_id: String,
    #[serde(rename = "senderNick", alias = "sender_nick")]
    pub sender_nick: String,
    #[serde(rename = "conversationId", alias = "conversation_id")]
    pub conversation_id: String,
    #[serde(rename = "conversationType", alias = "conversation_type")]
    pub conversation_type: String,
    #[serde(rename = "sessionWebhook", alias = "session_webhook")]
    pub session_webhook: String,
    pub content: Option<serde_json::Value>,
}

/// Text content wrapper.
#[derive(Debug, Deserialize)]
pub struct DingTalkTextContent {
    pub content: String,
}

/// DingTalk Stream Mode API response.
#[derive(Debug, Deserialize)]
struct StreamResponse {
    code: Option<i64>,
    message: Option<String>,
    data: Option<serde_json::Value>,
}

/// DingTalk Stream Mode event.
#[derive(Debug, Deserialize)]
struct StreamEvent {
    headers: Option<StreamEventHeaders>,
    data: Option<String>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamEventHeaders {
    #[serde(rename = "eventType")]
    event_type: Option<String>,
    #[serde(rename = "eventId")]
    event_id: Option<String>,
    #[serde(rename = "messageId")]
    message_id: Option<String>,
}

/// DingTalk conversation message payload.
#[derive(Debug, Deserialize)]
struct DingTalkConversationMessage {
    /// fix: DingTalk 真实 wire 格式是 camelCase
    /// （senderStaffId/senderNick/conversationId/conversationType）——同结构里
    /// sessionWebhook 早就 rename 了，这四个漏掉导致生产里全部解析成 None：
    /// sender_id 变 "unknown"、群消息 chat_id 塌缩、sender_nick 丢失。
    /// alias 保留 snake_case 兼容（既有内部测试用它构造）。
    #[serde(rename = "senderStaffId", alias = "sender_staff_id")]
    sender_staff_id: Option<String>,
    #[serde(rename = "senderNick", alias = "sender_nick")]
    sender_nick: Option<String>,
    #[serde(rename = "conversationId", alias = "conversation_id")]
    conversation_id: Option<String>,
    #[serde(rename = "conversationType", alias = "conversation_type")]
    conversation_type: Option<String>,
    #[serde(rename = "sessionWebhook")]
    session_webhook: Option<String>,
    text: Option<DingTalkTextContent>,
    content: Option<String>,
    #[serde(rename = "msgtype")]
    msg_type: Option<String>,
}

/// DingTalk access token response.
#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    expire_in: Option<i64>,
}

/// Markdown reply request.
#[derive(Serialize)]
struct MarkdownReplyRequest {
    msgtype: String,
    markdown: MarkdownContent,
}

/// Text reply request.
#[derive(Serialize)]
struct TextReplyRequest {
    msgtype: String,
    text: TextContent,
}

/// Markdown content.
#[derive(Serialize)]
struct MarkdownContent {
    title: String,
    text: String,
}

/// Text content.
#[derive(Serialize)]
struct TextContent {
    content: String,
}

/// DingTalk channel using Stream Mode.
pub struct DingTalkChannel {
    base: BaseChannel,
    config: DingTalkConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    /// Session webhook per chat (chat_id -> webhook URL).
    ///
    /// 必须 `Arc<DashMap>`——裸 `DashMap::clone()` 是逐
    /// shard 深拷贝，start() 交给流任务的副本与 send() 读的原表分叉，
    /// 会话 webhook 写入恒不可见（每条回复必失败）。
    session_webhooks: Arc<dashmap::DashMap<String, String>>,
    access_token: Arc<parking_lot::RwLock<String>>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// Cancellation sender for the stream loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl DingTalkChannel {
    /// Creates a new `DingTalkChannel`.
    pub fn new(
        config: DingTalkConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.client_id.is_empty() || config.client_secret.is_empty() {
            return Err(NemesisError::Channel(
                "dingtalk client_id and client_secret are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("dingtalk"),
            config,
            http: crate::base::outbound_http_client()?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            // 必须 Arc 包装——裸 DashMap 经 clone() 交给
            // 流任务的是深拷贝副本，任务侧写入的会话 webhook 对 send()
            // 读路径恒不可见（每条回复必失败）。
            session_webhooks: Arc::new(dashmap::DashMap::new()),
            access_token: Arc::new(parking_lot::RwLock::new(String::new())),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Obtains an access token from the DingTalk API.
    pub async fn refresh_token(&self) -> Result<String> {
        let params = serde_json::json!({
            "appKey": self.config.client_id,
            "appSecret": self.config.client_secret,
        });

        let resp = self
            .http
            .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("dingtalk auth failed: {e}")))?;

        let body: AccessTokenResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("dingtalk auth parse failed: {e}")))?;

        let token = body.access_token.unwrap_or_default();
        *self.access_token.write() = token.clone();
        Ok(token)
    }

    /// Stores a session webhook for a chat.
    pub fn store_session_webhook(&self, chat_id: String, webhook: String) {
        self.session_webhooks.insert(chat_id, webhook);
    }

    /// Sends a markdown reply via session webhook.
    pub async fn send_direct_reply(&self, session_webhook: &str, content: &str) -> Result<()> {
        let reply = MarkdownReplyRequest {
            msgtype: "markdown".to_string(),
            markdown: MarkdownContent {
                title: "NemesisBot".to_string(),
                text: content.to_string(),
            },
        };

        let resp = self
            .http
            .post(session_webhook)
            .json(&reply)
            .send()
            .await
            .map_err(|e| {
                // sessionWebhook URL 内嵌 access_token（官方形态），错误串须脱敏
                NemesisError::Channel(crate::base::redact_url_secrets(format!(
                    "dingtalk reply failed: {e}"
                )))
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "dingtalk reply error: {body}"
            )));
        }

        Ok(())
    }

    /// Sends a text reply via session webhook.
    pub async fn send_text_reply(&self, session_webhook: &str, content: &str) -> Result<()> {
        let reply = TextReplyRequest {
            msgtype: "text".to_string(),
            text: TextContent {
                content: content.to_string(),
            },
        };

        let resp = self
            .http
            .post(session_webhook)
            .json(&reply)
            .send()
            .await
            .map_err(|e| {
                // sessionWebhook URL 内嵌 access_token（官方形态），错误串须脱敏
                NemesisError::Channel(crate::base::redact_url_secrets(format!(
                    "dingtalk text reply failed: {e}"
                )))
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "dingtalk text reply error: {body}"
            )));
        }

        Ok(())
    }

    /// Processes a callback data and extracts content.
    pub fn extract_callback_content(data: &DingTalkCallbackData) -> &str {
        if !data.text.content.is_empty() {
            return &data.text.content;
        }
        ""
    }

    /// Parses a DingTalk stream event and publishes an InboundMessage.
    fn parse_and_dispatch_event(
        event: &StreamEvent,
        bus_sender: &broadcast::Sender<InboundMessage>,
        session_webhooks: &dashmap::DashMap<String, String>,
        allow_from: &[String],
    ) {
        let event_type = event
            .headers
            .as_ref()
            .and_then(|h| h.event_type.as_deref())
            .unwrap_or("");

        // Only handle conversation message events
        if event_type != "dingtalk.oapi.capi.conversation.message.receive" {
            debug!(event_type = %event_type, "[DingTalkChannel] stream: unhandled event type");
            return;
        }

        let payload_str = match &event.data {
            Some(d) => d,
            None => return,
        };

        let msg: DingTalkConversationMessage = match serde_json::from_str(payload_str) {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "[DingTalkChannel] stream: failed to parse message payload");
                return;
            }
        };

        let content = match &msg.text {
            Some(t) if !t.content.is_empty() => &t.content,
            _ => match &msg.content {
                Some(c) if !c.is_empty() => c,
                _ => return,
            },
        };

        if content.is_empty() {
            return;
        }

        let sender_id = msg
            .sender_staff_id
            .as_deref()
            .unwrap_or("unknown")
            .to_string();

        // Check allow list
        if !allow_from.is_empty() && !allow_from.contains(&sender_id) {
            debug!(sender_id = %sender_id, "[DingTalkChannel] message filtered by allow_list");
            return;
        }

        let conversation_id = msg.conversation_id.as_deref().unwrap_or("").to_string();

        let chat_id = if conversation_id.is_empty() {
            sender_id.clone()
        } else {
            conversation_id
        };

        // Store session webhook for replies
        if let Some(ref webhook) = msg.session_webhook.filter(|w| !w.is_empty()) {
            session_webhooks.insert(chat_id.clone(), webhook.clone());
        }

        let is_group = msg.conversation_type.as_deref().is_some_and(|t| t == "2");

        let mut metadata = std::collections::HashMap::new();
        if let Some(ref nick) = msg.sender_nick.filter(|n| !n.is_empty()) {
            metadata.insert("sender_nick".to_string(), nick.clone());
        }
        if let Some(ref msg_type) = msg.msg_type.filter(|t| !t.is_empty()) {
            metadata.insert("msg_type".to_string(), msg_type.clone());
        }
        metadata.insert(
            "is_group".to_string(),
            if is_group { "true" } else { "false" }.to_string(),
        );

        let inbound = InboundMessage {
            channel: "dingtalk".to_string(),
            sender_id: sender_id.clone(),
            chat_id: chat_id.clone(),
            content: content.clone(),
            media: Vec::new(),
            session_key: format!("dingtalk:{}", chat_id),
            correlation_id: String::new(),
            metadata,
            voice_playback: None,
        };

        info!(
            sender_id = %inbound.sender_id,
            chat_id = %inbound.chat_id,
            "[DingTalkChannel] received message"
        );

        if let Err(e) = bus_sender.send(inbound) {
            warn!("[DingTalkChannel] failed to publish inbound message: {e}");
        }
    }

    /// 循环尾退避等待：睡满 `backoff` 返回 true；等待期间取消被触发
    /// （oneshot 完成，含 sender 侧 drop）返回 false。
    /// 循环尾旧形态 `select! { sleep => {}, async {} => {} }`
    /// 空 async 臂首个 poll 即 Ready，select 恒立即返回，退避 sleep 被完全
    /// 短路（断连后零间隔重连锤击钉钉网关）。抽成独立函数以便单测锁语义。
    async fn backoff_wait(
        backoff: std::time::Duration,
        cancel_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(backoff) => true,
            _ = cancel_rx => false,
        }
    }

    /// poll 响应空载判定：204 No Content 或空 body = 网关无事件正常态
    /// （安静跳过，不进解析、不留痕）。
    fn poll_response_is_empty(status: u16, body: &str) -> bool {
        status == 204 || body.trim().is_empty()
    }

    /// Spawns the Stream Mode connection loop.
    fn spawn_stream_loop(&self) {
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        let client_id = self.config.client_id.clone();
        let client_secret = self.config.client_secret.clone();
        let running = self.running.clone();
        let http = self.http.clone();
        let bus_sender = self.bus_sender.clone();
        let session_webhooks = self.session_webhooks.clone();
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
                    "appKey": client_id,
                    "appSecret": client_secret,
                });

                let token = match http
                    .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
                    .json(&token_params)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if let Ok(body) = resp.json::<AccessTokenResponse>().await {
                            body.access_token.unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "[DingTalkChannel] auth failed, backing off");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = &mut cancel_rx => break,
                        }
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                if token.is_empty() {
                    warn!("[DingTalkChannel] auth returned empty token, backing off");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = &mut cancel_rx => break,
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }

                // Step 2: Open stream connection
                let connect_params = serde_json::json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "subscriptions": [
                        {
                            "type": "EVENT",
                            "topic": "/v1.0/im/bot/messages/get"
                        }
                    ],
                });

                match http
                    .post("https://api.dingtalk.com/v1.0/gateway/connections/open")
                    .header("X-Acs-Dingtalk-Access-Token", &token)
                    .json(&connect_params)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            if let Ok(body) = resp.json::<serde_json::Value>().await {
                                // Parse the stream connection response
                                // The response contains an endpoint for the event stream
                                if let Some(endpoint) = body
                                    .get("data")
                                    .and_then(|d| d.get("endpoint"))
                                    .and_then(|e| e.as_str())
                                {
                                    info!(endpoint = %endpoint, "[DingTalkChannel] stream connection opened");
                                    backoff = INITIAL_BACKOFF;

                                    // Poll the stream endpoint for events
                                    let poll_url =
                                        "https://api.dingtalk.com/v1.0/gateway/connections/poll";
                                    let mut poll_interval =
                                        tokio::time::interval(std::time::Duration::from_secs(1));
                                    // 默认 Burst 在挂起恢复后把错过的
                                    // tick 逐个补发——1s 轮询挂 2h 即对钉钉网关密集补发
                                    // 上万次 HTTP POST（配额打爆/封禁）。Delay 从当前
                                    // 时刻重排。
                                    poll_interval.set_missed_tick_behavior(
                                        tokio::time::MissedTickBehavior::Delay,
                                    );

                                    loop {
                                        tokio::select! {
                                            _ = &mut cancel_rx => {
                                                info!("[DingTalkChannel] stream loop shutting down");
                                                return;
                                            }
                                            _ = poll_interval.tick() => {
                                                if !*running.read() {
                                                    return;
                                                }

                                                match http
                                                    .post(poll_url)
                                                    .header("X-Acs-Dingtalk-Access-Token", &token)
                                                    .json(&serde_json::json!({
                                                        "endpoint": endpoint
                                                    }))
                                                    .send()
                                                    .await
                                                {
                                                    Ok(resp) if resp.status().is_success() => {
                                                        let status = resp.status();
                                                        let body_text =
                                                            resp.text().await.unwrap_or_default();
                                                        // 204 No Content / 空 body = 网关无事
                                                        // 件正常态，安静跳过——此前 204 落进
                                                        // 解析臂，空 body 双重解析失败 = 每秒
                                                        // 一条 "unparseable body, events
                                                        // dropped" 警告洪水（下方非 2xx 臂的
                                                        // 204 豁免是永假死条件）。
                                                        if Self::poll_response_is_empty(
                                                            status.as_u16(),
                                                            &body_text,
                                                        ) {
                                                            continue;
                                                        }
                                                        if let Ok(events) =
                                                            serde_json::from_str::<Vec<StreamEvent>>(
                                                                &body_text,
                                                            )
                                                        {
                                                            for event in &events {
                                                                Self::parse_and_dispatch_event(
                                                                    event,
                                                                    &bus_sender,
                                                                    &session_webhooks,
                                                                    &allow_from,
                                                                );
                                                            }
                                                        } else if let Ok(event) =
                                                            serde_json::from_str::<StreamEvent>(
                                                                &body_text,
                                                            )
                                                        {
                                                            Self::parse_and_dispatch_event(
                                                                &event,
                                                                &bus_sender,
                                                                &session_webhooks,
                                                                &allow_from,
                                                            );
                                                        } else {
                                                            // 畸形 200 body = 在飞事件静默丢失，必须留痕（对齐下方非 200 warn 风格）
                                                            warn!(
                                                                status = %status,
                                                                body = %body_text,
                                                                "[DingTalkChannel] stream poll returned unparseable body, events dropped"
                                                            );
                                                        }
                                                    }
                                                    Ok(resp) => {
                                                        let status = resp.status();
                                                        // 非 2xx = token 过期（401）或网关错
                                                        // 误：break 回外层重拿 token 重连——
                                                        // 此前只 warn 不 break，token 过期后
                                                        // 每秒 warn 且永不自愈，收件永久中断。
                                                        warn!(status = %status, "[DingTalkChannel] stream poll returned non-success, reconnecting");
                                                        break;
                                                    }
                                                    Err(e) => {
                                                        warn!(error = %e, "[DingTalkChannel] stream poll failed");
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    debug!(
                                        "[DingTalkChannel] stream connection response: {:?}",
                                        body
                                    );
                                    backoff = INITIAL_BACKOFF;
                                }
                            }
                        } else {
                            let status = resp.status();
                            let body = resp.text().await.unwrap_or_default();
                            warn!(status = %status, body = %body, "[DingTalkChannel] stream open failed");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "[DingTalkChannel] stream connect failed, backing off");
                    }
                }

                if !*running.read() {
                    break;
                }

                // 循环尾退避：旧形态 `select! { sleep => {}, async {} => {} }` 空
                // async 臂首个 poll 即 Ready，select 恒立即返回，退避 sleep 被完全
                // 短路（断连后零间隔重连）。外层所有失败路径（auth 失败/token 空/
                // stream open 失败/connect 失败）此前都经此位点零间隔重连。
                if !Self::backoff_wait(backoff, &mut cancel_rx).await {
                    break;
                }

                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("[DingTalkChannel] stream loop stopped");
        });
    }
}

#[async_trait]
impl Channel for DingTalkChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!("[DingTalkChannel] starting DingTalk channel (Stream Mode)");
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Try to get access token
        match self.refresh_token().await {
            Ok(token) => info!(token_len = token.len(), "[DingTalkChannel] authenticated"),
            Err(e) => warn!(error = %e, "[DingTalkChannel] auth failed (will retry)"),
        }

        // Start stream loop
        self.spawn_stream_loop();

        info!("[DingTalkChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[DingTalkChannel] stopping DingTalk channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        self.session_webhooks.clear();
        *self.access_token.write() = String::new();
        info!("[DingTalkChannel] channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "dingtalk channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let webhook = self
            .session_webhooks
            .get(&msg.chat_id)
            .map(|w| w.value().clone())
            .ok_or_else(|| {
                NemesisError::Channel(format!("no session_webhook found for chat {}", msg.chat_id))
            })?;

        debug!(chat_id = %msg.chat_id, "[DingTalkChannel] sending message");
        self.send_direct_reply(&webhook, &msg.content).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

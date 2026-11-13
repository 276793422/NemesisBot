//! QQ bot channel (REST API + WebSocket, C2C/group messages).
//!
//! Uses the QQ Bot REST API for sending messages and WebSocket for receiving.
//! Supports C2C and group @-mention messages with deduplication.

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
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
/// SEC-002（BUG-20261111-068）：app_secret 脱敏。
#[derive(Clone)]
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

crate::debug_redact::impl_redacted_debug!(QQConfig {
    plain { app_id, api_base, allow_from }
    secret { app_secret }
});

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
    pub fn new(config: QQConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.app_id.is_empty() || config.app_secret.is_empty() {
            return Err(NemesisError::Channel(
                "QQ app_id and app_secret are required".to_string(),
            ));
        }

        // api_base 尾斜杠归一：config 反序列化直通，用户从文档复制带尾
        // 斜杠的 base 会拼出 `//app` 双斜杠路径（上游归一行为不可赌，404
        // 即通道整体断链）。同族先例 = bluesky/mastodon 构造期归一
        // （BUG-20261109-012）。构造期写回 = 5 个 api_base 消费点（含
        // spawn 闭包捕获）单一归一漏斗。
        let config = QQConfig {
            api_base: config.api_base.trim_end_matches('/').to_string(),
            ..config
        };

        Ok(Self {
            base: BaseChannel::new("qq"),
            config,
            http: crate::base::outbound_http_client()?,
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

    /// 循环尾退避等待：睡满 `backoff` 返回 true；等待期间取消被触发
    /// （oneshot 完成，含 sender 侧 drop）返回 false。
    /// 循环尾旧形态 `select! { sleep => {}, async {} => {} }`
    /// 空 async 臂首个 poll 即 Ready，select 恒立即返回，退避 sleep 被完全
    /// 短路（断连后零间隔重连锤击 QQ 网关）。抽成独立函数以便单测锁语义。
    async fn backoff_wait(
        backoff: std::time::Duration,
        cancel_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(backoff) => true,
            _ = cancel_rx => false,
        }
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

        // status 闸（R736）：非 2xx 的失败响应体是合法 JSON（无 access_token
        // 字段），typed 反序列化成功 + `unwrap_or_default()` 会把 auth 失败
        // 吞成 Ok(空串)——HTTP 真因（401/403）消失，下游只剩误导性的
        // "QQ access token not available"。显式回报 HTTP 状态。
        let status = resp.status();
        let body: AccessTokenResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("QQ auth parse failed: {e}")))?;
        if !status.is_success() {
            return Err(NemesisError::Channel(format!(
                "QQ auth failed: HTTP {status}"
            )));
        }

        let token = body.access_token.filter(|t| !t.is_empty()).ok_or_else(|| {
            NemesisError::Channel("QQ auth: no access_token in response".to_string())
        })?;
        *self.access_token.write() = token.clone();
        Ok(token)
    }

    /// Sends a C2C message via QQ REST API.
    ///
    /// token 失效自愈骨架包装：见 [`Self::send_with_fresh_token`]。
    pub async fn send_c2c_message(&self, openid: &str, content: &str) -> Result<()> {
        self.send_with_fresh_token(|| self.send_c2c_once(openid, content))
            .await
    }

    /// 单次 C2C 发送尝试（token 自愈重试的原始请求）。
    async fn send_c2c_once(&self, openid: &str, content: &str) -> Result<()> {
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

        let url = format!("{}/v2/users/{}/messages", self.config.api_base, openid);

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("QQ C2C send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = nemesis_utils::http_cap::read_error_body(resp).await;
            return Err(NemesisError::Channel(format!("QQ C2C send error: {body}")));
        }

        Ok(())
    }

    /// Sends a group message via QQ REST API.
    ///
    /// token 失效自愈骨架包装：见 [`Self::send_with_fresh_token`]。
    pub async fn send_group_message(&self, group_openid: &str, content: &str) -> Result<()> {
        self.send_with_fresh_token(|| self.send_group_once(group_openid, content))
            .await
    }

    /// 单次群发送尝试（token 自愈重试的原始请求）。
    async fn send_group_once(&self, group_openid: &str, content: &str) -> Result<()> {
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
            let body = nemesis_utils::http_cap::read_error_body(resp).await;
            return Err(NemesisError::Channel(format!(
                "QQ group send error: {body}"
            )));
        }

        Ok(())
    }

    /// 出站 token 自愈骨架：QQ access_token 有效期约 2h，共享槽唯一写入
    /// 点 = start() 的一次 refresh（receive loop 内获取的 token 是局部
    /// 变量，不写回槽）——过期后出站恒被拒且无任何刷新机制。现发送前
    /// 空槽惰性补种；发送被拒（HTTP 非 2xx）刷新一次 token 重试一次；
    /// 重试仍失败保留原始错误（非 token 类失败如 openid 不存在多打一次
    /// 请求，无害）。
    async fn send_with_fresh_token<F, Fut>(&self, send_once: F) -> Result<()>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        if self.access_token.read().is_empty() {
            self.refresh_token().await?;
        }
        match send_once().await {
            Err(e) if Self::is_send_rejected(&e) => {
                debug!(error = %e, "[QQChannel] send rejected, refreshing token and retrying once");
                self.refresh_token().await?;
                send_once().await
            }
            r => r,
        }
    }

    /// 发送被拒判定：HTTP 非 2xx（token 过期的 wire 形态）——传输层错误
    /// 前缀不同（"QQ ... send failed"）与空槽错误（"QQ access token not
    /// available"）不触发重试。
    fn is_send_rejected(e: &NemesisError) -> bool {
        matches!(
            e,
            NemesisError::Channel(m)
                if m.starts_with("QQ C2C send error") || m.starts_with("QQ group send error")
        )
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
                        // 默认 Burst 在挂起恢复后把错过的心跳
                        // 逐个补发——对端被心跳洪泛。Delay 从当前时刻重排。
                        heartbeat_interval
                            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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

                // 循环尾退避：旧形态 `select! { sleep => {}, async {} => {} }` 空
                // async 臂首个 poll 即 Ready，select 恒立即返回，退避 sleep 被完全
                // 短路（断连后零间隔重连）。外层所有失败路径（connect 失败/WS 流
                // 结束/服务端 Close）此前都经此位点零间隔重连。
                if !Self::backoff_wait(backoff, &mut cancel_rx).await {
                    break;
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
            voice_playback: None,
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
            voice_playback: None,
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

    fn is_running(&self) -> bool {
        self.base.is_running()
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

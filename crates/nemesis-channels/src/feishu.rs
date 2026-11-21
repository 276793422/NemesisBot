//! Feishu/Lark channel (event callback, WebSocket mode, text messages).
//!
//! Uses the Feishu/Lark Open Platform SDK via WebSocket for receiving
//! events and the REST API for sending messages. Falls back to HTTP
//! event polling when the WebSocket endpoint is not available.

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel, EventDedup};

/// Initial backoff duration for reconnect.
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
/// Maximum backoff duration.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
/// Default WebSocket ping interval in seconds.
const DEFAULT_WS_PING_INTERVAL_SECS: u64 = 30;
/// 单帧写超时上限（秒）。BUG-20261115-016：半死连接上无界写 await 会冻结
/// 整个读循环（select 单臂卡住 = Pong/Ping/关停全停），有界超时让既有
/// break/return 语义可达，重连臂接管。家族值对齐 IRC WRITE_TIMEOUT_SECS。
const WS_WRITE_TIMEOUT_SECS: u64 = 30;

/// 默认 API 域名（国内飞书）。BUG-20261121-045：模块宣称「Feishu/Lark
/// channel」但此前全部 5 处 URL 硬编码 open.feishu.cn——国际版 Lark
/// （open.larksuite.com，同 API 形态）用户完全不可用（宣称与代码事实族，
/// 同 BUG-036/037/040）。现 api_base 可配：config 缺省/空 = 此默认。
pub const DEFAULT_API_BASE: &str = "https://open.feishu.cn";

/// Feishu channel configuration.
/// SEC-002（BUG-20261111-068）：app_secret / verification_token /
/// encrypt_key 脱敏（app_id 保留诊断价值）。
#[derive(Clone)]
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
    /// API base（BUG-20261121-045）：空/缺省 = [`DEFAULT_API_BASE`]；
    /// Lark 国际版配 `https://open.larksuite.com`。
    pub api_base: String,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_secret: String::new(),
            verification_token: String::new(),
            encrypt_key: String::new(),
            allow_from: Vec::new(),
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }
}

crate::debug_redact::impl_redacted_debug!(FeishuConfig {
    plain { app_id, allow_from }
    secret { app_secret, verification_token, encrypt_key }
});

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

// EventDedup 已提升至 crate::base（BUG-20261121-051：单一真相源，slack
// BUG-20261121-051 同批共享；本文件经 use crate::base::EventDedup 使用）。

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
    /// 入站事件去重（BUG-20261121-048）：按 header.event_id。
    event_dedup: Arc<EventDedup>,
}

impl FeishuChannel {
    /// Creates a new `FeishuChannel`.
    pub fn new(
        config: FeishuConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        Self::new_with_client(config, bus_sender, crate::base::outbound_http_client()?)
    }

    /// 可注入 HTTP client 的构造器（家族惯例：discord/telegram 同款——
    /// 回归锁 loopback mock 零外网依赖的前置基建）。
    pub fn new_with_client(
        mut config: FeishuConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
        http: reqwest::Client,
    ) -> Result<Self> {
        if config.app_id.is_empty() || config.app_secret.is_empty() {
            return Err(NemesisError::Channel(
                "feishu app_id and app_secret are required".to_string(),
            ));
        }

        // BUG-20261121-045：api_base 单一漏斗——空/缺省回落官方默认
        // （Lark 国际版经 config 配 open.larksuite.com），尾斜杠构造期
        // 归一（discord BUG-20261109-012 同款：双斜杠路径会让全部 API
        // 断链）。
        if config.api_base.is_empty() {
            config.api_base = DEFAULT_API_BASE.to_string();
        }
        config.api_base = config.api_base.trim_end_matches('/').to_string();

        Ok(Self {
            base: BaseChannel::new("feishu"),
            config,
            http,
            running: Arc::new(parking_lot::RwLock::new(false)),
            access_token: Arc::new(parking_lot::RwLock::new(String::new())),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
            event_dedup: Arc::new(EventDedup::new()),
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
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                self.config.api_base
            ))
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

        let token = body.data.map(|d| d.tenant_access_token).unwrap_or_default();
        *self.access_token.write() = token.clone();
        Ok(token)
    }

    /// Sends a text message to a Feishu chat.
    ///
    /// token 失效自愈骨架包装：见 [`Self::send_with_fresh_token`]。
    pub async fn send_text_message(&self, chat_id: &str, text: &str) -> Result<()> {
        self.send_with_fresh_token(|| self.send_text_once(chat_id, text))
            .await
    }

    /// 单次发送尝试（token 自愈重试的原始请求）。
    async fn send_text_once(&self, chat_id: &str, text: &str) -> Result<()> {
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
            .post(format!("{}/open-apis/im/v1/messages", self.config.api_base))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("feishu send failed: {e}")))?;

        // BUG-20261121-047：飞书 OpenAPI 响应体统一形态 {"code": i64,
        // "msg": str, "data": ...}，「code 非 0 表示失败」（官方文档）——
        // 业务拒绝（机器人不在群 230002 / 用户不可用 230013 / 限频 230020 /
        // 缺权限 230027 等）可以 HTTP 2xx + code!=0 形态出现，此前只查
        // HTTP status 会把发送失败静默当成功（agent 假报已回复，用户收
        // 不到消息）。code 判定优先于 status；解析失败（HTML 错误页等
        // 非 JSON 形态）也诚实报错而非当成功。全部失败路径保持
        // "feishu send error" 前缀（is_send_rejected 契约面：token 类
        // 失败触发刷新重试一次）。body 截断防错误页打爆日志。
        let status = resp.status();
        // BUG-20261121-056：send 成功体有帽读取（BUG-20261115-024 挂账收尾：
        // 通道出站面成功体裸 text() 是 024 横向排查「后续轮收口」名单成员——
        // 失控端点 200 态灌内存的面与 provider 同构）。4MB 帽 = send 回执
        // 合法载荷（KB 级）× 大余量；超帽读入中途断开。错误映射保持
        // "feishu send error" 前缀契约（is_send_rejected 只认 token 类
        // 子串，超帽/传输失败不触发刷新重试）。
        const SEND_BODY_CAP: u64 = 4 * 1024 * 1024;
        let body_text = nemesis_utils::http_cap::read_body_capped(resp, SEND_BODY_CAP)
            .await
            .map_err(|e| {
                NemesisError::Channel(format!(
                    "feishu send error: HTTP {status}: read body failed: {e}"
                ))
            })
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())?;
        let capped: String = body_text.chars().take(1024).collect();
        let parsed: std::result::Result<ApiResponse<serde_json::Value>, _> =
            serde_json::from_str(&body_text);
        match parsed {
            Ok(body) if body.code != 0 => Err(NemesisError::Channel(format!(
                "feishu send error: code={} msg={}",
                body.code,
                body.msg.unwrap_or_default()
            ))),
            Ok(_) if !status.is_success() => Err(NemesisError::Channel(format!(
                "feishu send error: HTTP {status}: {capped}"
            ))),
            Ok(_) => Ok(()),
            Err(e) => Err(NemesisError::Channel(format!(
                "feishu send error: HTTP {status}: body parse failed: {e}: {capped}"
            ))),
        }
    }

    /// 出站 token 自愈骨架：tenant_access_token 有效期约 2h，共享槽唯一
    /// 写入点 = start() 的一次 refresh（receive loop 内获取的 token 是局部
    /// 变量，不写回槽）——过期后出站恒被拒且无任何刷新机制。现发送前
    /// 空槽惰性补种（start() 时 auth 失败只 warn 不阻断启动，槽可能一直
    /// 为空）；发送被拒（HTTP 非 2xx）刷新一次 token 重试一次；重试仍
    /// 失败保留原始错误（非 token 类失败如 chat_id 不存在多打一次请求，
    /// 无害）。
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
                debug!(error = %e, "[FeishuChannel] send rejected, refreshing token and retrying once");
                self.refresh_token().await?;
                send_once().await
            }
            r => r,
        }
    }

    /// 发送被拒判定：HTTP 非 2xx（token 过期的 wire 形态）——传输层错误
    /// 前缀不同（"feishu send failed"）与空槽错误（"no access token
    /// available"）不触发重试。
    fn is_send_rejected(e: &NemesisError) -> bool {
        matches!(e, NemesisError::Channel(m) if m.starts_with("feishu send error"))
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
            .post(format!("{}/callback/ws/endpoint", self.config.api_base))
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
        dedup: &EventDedup,
    ) {
        // BUG-20261121-048：at-least-once 重推去重——header.event_id 在场
        // 时查重（重复 = 平台重推，静默丢弃）；缺失（schema 1.0 老形态/
        // 非 v2 事件）保守放行，宁重复不丢。
        if let Some(id) = event_json
            .get("header")
            .and_then(|h| h.get("event_id"))
            .and_then(|v| v.as_str())
            && !id.is_empty()
            && !dedup.check_and_record(id)
        {
            debug!(event_id = %id, "[FeishuChannel] duplicate event dropped (at-least-once redelivery)");
            return;
        }

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
            debug!(sender_id = %sender_id, "[FeishuChannel] message filtered by allow_list");
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
            voice_playback: None,
        };

        info!(
            sender_id = %inbound.sender_id,
            chat_id = %inbound.chat_id,
            "[FeishuChannel] received message"
        );

        if let Err(e) = bus_sender.send(inbound) {
            warn!("[FeishuChannel] failed to publish inbound message: {e}");
        }
    }

    /// 循环尾退避等待：睡满 `backoff` 返回 true；等待期间取消被触发
    /// （oneshot 完成，含 sender 侧 drop）返回 false。
    /// 循环尾旧形态 `select! { sleep => {}, async {} => {} }`
    /// 空 async 臂首个 poll 即 Ready，select 恒立即返回，退避 sleep 被完全
    /// 短路（断连后零间隔重连）。抽成独立函数以便单测锁语义。
    async fn backoff_wait(
        backoff: std::time::Duration,
        cancel_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(backoff) => true,
            _ = cancel_rx => false,
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
        let event_dedup = self.event_dedup.clone();

        tokio::spawn(async move {
            let mut cancel_rx = cancel_rx;
            let mut backoff = INITIAL_BACKOFF;

            loop {
                if !*running.read() {
                    break;
                }

                // BUG-20261121-049：旧 Step 1（auth 取 token 存 `_token`）
                // 已删除——token 从未被消费（endpoint API 用 AppID/AppSecret
                // in-body 鉴权、WS 连接无 token 依赖），每轮重连白打一次
                // auth 请求；token 的实际生产路径 = start() 的一次 refresh
                // + 出站发送空槽惰性补种（send_with_fresh_token）。

                // Step 2: Get WebSocket endpoint
                let endpoint_result = http
                    .post(format!("{}/callback/ws/endpoint", config.api_base))
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
                                    warn!("[FeishuChannel] ws endpoint: missing data, backing off");
                                    tokio::select! {
                                        _ = tokio::time::sleep(backoff) => {}
                                        _ = &mut cancel_rx => break,
                                    }
                                    backoff = (backoff * 2).min(MAX_BACKOFF);
                                    continue;
                                }
                            } else {
                                warn!(
                                    code = body.code,
                                    "[FeishuChannel] ws endpoint error, backing off"
                                );
                                tokio::select! {
                                    _ = tokio::time::sleep(backoff) => {}
                                    _ = &mut cancel_rx => break,
                                }
                                backoff = (backoff * 2).min(MAX_BACKOFF);
                                continue;
                            }
                        } else {
                            warn!("[FeishuChannel] ws endpoint parse failed, backing off");
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut cancel_rx => break,
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "[FeishuChannel] ws endpoint request failed, backing off");
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = &mut cancel_rx => break,
                        }
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                if ws_url.is_empty() {
                    warn!("[FeishuChannel] ws endpoint returned empty URL, backing off");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = &mut cancel_rx => break,
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }

                // ws_url 为 endpoint API 原样返回（wss://…?ticket=<连接凭据>
                // 形态）——裸打即凭据进日志（BUG-20261119-182）。
                info!(url = %crate::base::redact_url_secrets(&ws_url), "[FeishuChannel] connecting to WebSocket endpoint");

                // Step 3: Connect to WebSocket
                match crate::base::ws_connect_bounded(&ws_url).await {
                    Ok((ws_stream, _)) => {
                        info!("[FeishuChannel] WebSocket connected successfully");
                        backoff = INITIAL_BACKOFF;

                        let (mut write, mut read) = ws_stream.split();
                        let mut ping_interval_timer =
                            tokio::time::interval(std::time::Duration::from_secs(ping_interval));
                        // 默认 Burst 在挂起恢复后把错过的 ping
                        // 逐个补发——对端被 ping 洪泛。Delay 从当前时刻重排。
                        ping_interval_timer
                            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
                                                    &event_dedup,
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
                                                        &event_dedup,
                                                    );
                                                }
                                            } else {
                                                // Try parsing the whole binary as JSON (fallback)
                                                if let Ok(event_json) = serde_json::from_slice::<serde_json::Value>(&data) {
                                                    Self::parse_and_publish_event(
                                                        &event_json,
                                                        &bus_sender,
                                                        &allow_from,
                                                        &event_dedup,
                                                    );
                                                }
                                            }
                                        }
                                        Some(Ok(Message::Ping(payload))) => {
                                            // BUG-20261115-016：有界写，超时静默放弃本帧
                                            let _ = tokio::time::timeout(
                                                std::time::Duration::from_secs(WS_WRITE_TIMEOUT_SECS),
                                                write.send(Message::Pong(payload)),
                                            )
                                            .await;
                                        }
                                        Some(Ok(Message::Close(frame))) => {
                                            info!(frame = ?frame, "[FeishuChannel] WebSocket closed by server");
                                            break;
                                        }
                                        Some(Ok(Message::Pong(_))) => {
                                            debug!("[FeishuChannel] WebSocket pong received");
                                        }
                                        Some(Ok(_)) => {}
                                        Some(Err(e)) => {
                                            warn!(error = %e, "[FeishuChannel] WebSocket stream error");
                                            break;
                                        }
                                        None => {
                                            info!("[FeishuChannel] WebSocket stream ended");
                                            break;
                                        }
                                    }
                                }
                                _ = ping_interval_timer.tick() => {
                                    // Send a text ping (Feishu accepts text-based pings)
                                    // BUG-20261115-016：有界写，超时与发送失败同走 break 重连
                                    if tokio::time::timeout(
                                        std::time::Duration::from_secs(WS_WRITE_TIMEOUT_SECS),
                                        write.send(Message::Ping(vec![].into())),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        warn!("[FeishuChannel] WebSocket ping failed");
                                        break;
                                    }
                                }
                                _ = &mut cancel_rx => {
                                    info!("[FeishuChannel] WebSocket receive loop shutting down");
                                    // BUG-20261115-016：关停路径 close 也有界（挂死连接上 close 同样可永远 pending）
                                    let _ = tokio::time::timeout(
                                        std::time::Duration::from_secs(WS_WRITE_TIMEOUT_SECS),
                                        write.close(),
                                    )
                                    .await;
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "[FeishuChannel] WebSocket connect failed, backing off");
                    }
                }

                // cancel_rx 在取消路径经 return/break 退出循环，到循环尾
                // 并未被消费（&mut 借用 cancel-safe），仍可与退避 sleep
                // 一起 select——与循环内其余退避点同款形态。
                if !*running.read() {
                    break;
                }

                if !Self::backoff_wait(backoff, &mut cancel_rx).await {
                    break;
                }

                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("[FeishuChannel] receive loop stopped");
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

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        // BUG-20261121-046：幂等双启守卫（telegram BUG-20261110-037 /
        // discord BUG-20261121-043 同构第三实锤）——无守卫的重复 start
        // 会 spawn 多个并行 receive loop，各自连 WS 各自发布 inbound，
        // 每条消息 N 倍喂 agent。
        if self.base.is_running() {
            info!("[FeishuChannel] already running, skipping duplicate start");
            return Ok(());
        }

        info!("[FeishuChannel] starting Feishu channel (WebSocket mode)");
        *self.running.write() = true;
        // BUG-20261121-046：trait is_running()（读 base 旗标）此前恒 false，
        // 违反「start 成功后、stop 前为 running」契约——manager 的运行中
        // 守卫（ChannelManager::start_all 读 trait is_running）对本通道失效。
        self.base.set_running(true);
        self.base.set_enabled(true);

        // Try to obtain access token。失败不阻断启动：出站发送时
        // send_with_fresh_token 空槽惰性补种（BUG-20261121-049：旧文案
        // 「will retry in receive loop」不实——receive loop 已无 auth 步，
        // token 从不在此消费）。
        match self.refresh_token().await {
            Ok(token) => info!(token_len = token.len(), "[FeishuChannel] authenticated"),
            Err(e) => {
                warn!(error = %e, "[FeishuChannel] auth failed (outbound sends will lazily refresh token)")
            }
        }

        // Start the WebSocket receive loop
        self.spawn_receive_loop();

        info!("[FeishuChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[FeishuChannel] stopping Feishu channel");
        *self.running.write() = false;
        // BUG-20261121-046：对称清 base 旗标（契约面见 start 内注释）。
        self.base.set_running(false);
        self.base.set_enabled(false);

        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        *self.access_token.write() = String::new();
        info!("[FeishuChannel] channel stopped");
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
        debug!(chat_id = %msg.chat_id, "[FeishuChannel] sending message");
        self.send_text_message(&msg.chat_id, &msg.content).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

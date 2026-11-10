//! LINE Messaging API channel (webhook, signature verification, reply tokens).
//!
//! Receives webhook events via HTTP POST and verifies `X-Line-Signature`
//! (base64 HMAC-SHA256) **fail-closed**——缺签名头/验签不过一律 401，绝不放行
//! 未验签流量；发送走 Messaging API（reply token 优先，回落 push）。

use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};
use nemesis_types::utils::split_message;

use crate::base::{BaseChannel, Channel};

type HmacSha256 = Hmac<Sha256>;

/// Webhook 请求路径默认值（与 config 模板 `channels.line.webhook_path` 同值）。
pub const DEFAULT_WEBHOOK_PATH: &str = "/webhook/line";

/// LINE channel configuration.
/// SEC-002（BUG-20261111-068）：channel_access_token / channel_secret 脱敏。
#[derive(Clone)]
pub struct LineConfig {
    /// Channel access token.
    pub channel_access_token: String,
    /// Channel secret (for signature verification).
    pub channel_secret: String,
    /// Webhook 服务器 bind 主机（空值回落 "0.0.0.0"；反代/隧道场景可配
    /// "127.0.0.1" 只听回环）。此前该配置侧字段存在但通道侧无对应字段、
    /// bind 硬编码 "0.0.0.0"——声名未读取的死键（BUG-20261110-055）。
    pub webhook_host: String,
    /// Webhook server listen port.
    pub webhook_port: u16,
    /// Webhook 请求路径（LINE 控制台回调 URL 的路径段）。请求路径不匹配
    /// → 404（wechat `callback_path` 同款家族形态）。此前请求路径从不
    /// 检查（任意路径照常验签处理）——声名未读取的死键（BUG-20261110-055）。
    pub webhook_path: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

crate::debug_redact::impl_redacted_debug!(LineConfig {
    plain { webhook_host, webhook_port, webhook_path, allow_from }
    secret { channel_access_token, channel_secret }
});

impl LineConfig {
    /// bind 主机（trim；空值回落 "0.0.0.0" 保持旧行为）。
    pub fn webhook_host_resolved(&self) -> &str {
        let h = self.webhook_host.trim();
        if h.is_empty() { "0.0.0.0" } else { h }
    }

    /// 回调路径（trim；空值回落 [`DEFAULT_WEBHOOK_PATH`]；保证以 `/` 开头）。
    pub fn webhook_path_resolved(&self) -> String {
        let p = self.webhook_path.trim();
        if p.is_empty() {
            DEFAULT_WEBHOOK_PATH.to_string()
        } else if p.starts_with('/') {
            p.to_string()
        } else {
            format!("/{p}")
        }
    }
}

/// LINE webhook request body.
#[derive(Debug, serde::Deserialize)]
pub struct LineWebhookRequest {
    pub destination: Option<String>,
    pub events: Vec<LineEvent>,
}

/// A single LINE event.
#[derive(Debug, serde::Deserialize)]
pub struct LineEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(rename = "replyToken")]
    pub reply_token: Option<String>,
    pub source: Option<LineSource>,
    pub message: Option<LineMessage>,
    pub timestamp: Option<i64>,
}

/// LINE event source.
#[derive(Debug, serde::Deserialize)]
pub struct LineSource {
    #[serde(rename = "type")]
    pub source_type: String,
    /// LINE webhook JSON uses camelCase keys (`userId`/`groupId`/`roomId`).
    /// fix: 缺 rename 时反序列化得到 None，
    /// 导致所有入站消息 sender_id/chat_id 变成 "unknown"。
    #[serde(rename = "userId", alias = "user_id")]
    pub user_id: Option<String>,
    #[serde(rename = "groupId", alias = "group_id")]
    pub group_id: Option<String>,
    #[serde(rename = "roomId", alias = "room_id")]
    pub room_id: Option<String>,
}

/// LINE message content.
#[derive(Debug, serde::Deserialize)]
pub struct LineMessage {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub message_type: String,
    pub text: Option<String>,
}

/// LINE reply message request.
#[derive(serde::Serialize)]
struct LineReplyRequest {
    reply_token: String,
    messages: Vec<LineMessagePayload>,
}

/// LINE push message request.
#[derive(serde::Serialize)]
struct LinePushRequest {
    to: String,
    messages: Vec<LineMessagePayload>,
}

/// A LINE message payload.
#[derive(serde::Serialize)]
struct LineMessagePayload {
    #[serde(rename = "type")]
    msg_type: String,
    text: String,
}

/// LINE channel using Messaging API.
pub struct LineChannel {
    base: BaseChannel,
    config: LineConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    reply_tokens: Arc<dashmap::DashMap<String, String>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl LineChannel {
    /// Creates a new `LineChannel`.
    pub fn new(config: LineConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.channel_access_token.is_empty() || config.channel_secret.is_empty() {
            return Err(NemesisError::Channel(
                "LINE channel_access_token and channel_secret are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("line"),
            config,
            http: crate::base::outbound_http_client()?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            reply_tokens: Arc::new(dashmap::DashMap::new()),
            bus_sender,
        })
    }

    /// Verifies the webhook signature (`X-Line-Signature`).
    ///
    /// LINE 平台的实际线上格式是 **base64**(HMAC-SHA256(body))（官方 SDK 与
    /// 文档一致），不是 hex——旧实现按 hex 解码，对真实请求恒拒签且零生产
    /// 调用方（死代码陷阱）。现统一走 [`Self::verify_signature_with_secret`]
    /// 单一真相源（base64 解码 + 常时比较）。
    pub fn verify_signature(&self, body: &[u8], signature: &str) -> bool {
        Self::verify_signature_with_secret(&self.config.channel_secret, body, signature)
    }

    /// 签名校验统一定义：`signature` 必须等于 base64(HMAC-SHA256(body,
    /// secret))；解码失败（空串/非法字符）与长度不符一律拒绝；比较为常时
    /// 逐字节 XOR，不给时序侧信道留口子。`verify_signature`（&self 面）与
    /// `handle_webhook_connection`（secret 参数面）都从这里走，杜绝双实现漂移。
    fn verify_signature_with_secret(secret: &str, body: &[u8], signature: &str) -> bool {
        let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(body);
        let expected = mac.finalize().into_bytes();

        base64::engine::general_purpose::STANDARD
            .decode(signature)
            .ok()
            .is_some_and(|sig| {
                let expected_bytes = expected.as_slice();
                if sig.len() != expected_bytes.len() {
                    return false;
                }
                // Constant-time comparison
                let mut result = 0u8;
                for (a, b) in sig.iter().zip(expected_bytes.iter()) {
                    result |= a ^ b;
                }
                result == 0
            })
    }

    /// Stores a reply token for a chat.
    pub fn store_reply_token(&self, chat_id: String, reply_token: String) {
        self.reply_tokens.insert(chat_id, reply_token);
    }

    /// Sends a reply using a stored reply token.
    pub async fn reply(&self, reply_token: &str, text: &str) -> Result<()> {
        let request = LineReplyRequest {
            reply_token: reply_token.to_string(),
            messages: vec![LineMessagePayload {
                msg_type: "text".to_string(),
                text: text.to_string(),
            }],
        };

        let resp = self
            .http
            .post("https://api.line.me/v2/bot/message/reply")
            .header(
                "Authorization",
                format!("Bearer {}", self.config.channel_access_token),
            )
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("LINE reply failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!("LINE reply error: {body}")));
        }

        Ok(())
    }

    /// Sends a push message to a user/group/room.
    pub async fn push_message(&self, to: &str, text: &str) -> Result<()> {
        let request = LinePushRequest {
            to: to.to_string(),
            messages: vec![LineMessagePayload {
                msg_type: "text".to_string(),
                text: text.to_string(),
            }],
        };

        let resp = self
            .http
            .post("https://api.line.me/v2/bot/message/push")
            .header(
                "Authorization",
                format!("Bearer {}", self.config.channel_access_token),
            )
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("LINE push failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!("LINE push error: {body}")));
        }

        Ok(())
    }

    /// Handles a single webhook TCP connection with minimal HTTP parsing.
    async fn handle_webhook_connection(
        stream: tokio::net::TcpStream,
        bus: &broadcast::Sender<InboundMessage>,
        channel_secret: &str,
        webhook_path: &str,
        reply_tokens: &Arc<dashmap::DashMap<String, String>>,
        running: &Arc<parking_lot::RwLock<bool>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let _ = reply_tokens; // used via DashMap clone

        if !*running.read() {
            return;
        }

        let mut buf = vec![0u8; 65536];
        let mut stream = stream;
        let n = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let request_data = &buf[..n];
        let request_str = String::from_utf8_lossy(request_data);

        // 路径闸（wechat.rs:577 callback_path 同款家族形态）：请求路径与
        // 配置不匹配 → 404。此前 server 对任意路径的 POST 都照常处理——
        // config `webhook_path` 是声名未读取的死键（BUG-20261110-055），
        // 用户在 LINE 控制台/配置里对齐的路径毫无约束力。取请求行第二段
        // （strip query string）；闸在验签之前（错路径无需先做 HMAC）。
        let req_path = request_str
            .lines()
            .next()
            .and_then(|l| l.split_ascii_whitespace().nth(1))
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        if req_path != webhook_path {
            warn!(
                "[LineChannel] webhook path mismatch: got `{req_path}`, expected \
                 `{webhook_path}` — 请对齐 LINE 控制台回调 URL 与 config \
                 channels.line.webhook_path"
            );
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        // Extract X-Line-Signature header：扫整个 header 区（到空行为止）。
        // 旧 take(20) 行数上限会把带较多头部的真实请求误判成缺签名；头部名
        // 按 HTTP 语义 ASCII 大小写不敏感匹配（旧实现只认两种拼法）。
        let mut signature = "";
        for line in request_str.lines() {
            if line.is_empty() {
                break; // header/body 分隔空行，header 区结束
            }
            if let Some((name, val)) = line.split_once(':')
                && name.eq_ignore_ascii_case("x-line-signature")
            {
                signature = val.trim();
                break;
            }
        }

        // Find body after double CRLF——按**原始字节**定位（windows 扫描），
        // 不用 lossy 字符串偏移切原始字节：header 区含非 UTF-8 字节时
        // from_utf8_lossy 的替换膨胀会让字符串偏移大于原始字节偏移，
        // 切片越界 panic（可远程无认证触发——panic 点在验签之前）。
        // wechat.rs handle_connection 同款纪律。
        let body = match request_data.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(idx) => &request_data[idx + 4..n],
            None => &request_data[..n],
        };

        // 签名闸（fail-closed）：LINE 平台发出的每个 webhook 都带
        // X-Line-Signature；缺头/空头的请求不可能来自 LINE。旧形态静默放行
        // （200 + 直接入总线），任何能摸到监听端口的进程都能伪造
        // userId/chat_id 投毒。
        if signature.is_empty() {
            warn!("[LineChannel] webhook request missing X-Line-Signature; rejecting");
            let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        // Verify signature（统一定义：base64 + 常时比较）
        if !Self::verify_signature_with_secret(channel_secret, body, signature) {
            warn!("[LineChannel] invalid webhook signature");
            let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        // Parse webhook body
        let webhook: LineWebhookRequest = match serde_json::from_slice(body) {
            Ok(w) => w,
            Err(e) => {
                warn!("[LineChannel] failed to parse webhook body: {e}");
                let resp = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                return;
            }
        };

        // Send 200 OK
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes()).await;

        // Process events
        for event in &webhook.events {
            if event.event_type != "message" {
                continue;
            }

            let msg = match event.message.as_ref() {
                Some(m) => m,
                None => continue,
            };

            let text = match msg.text.as_deref() {
                Some(t) if !t.is_empty() => t,
                _ => continue,
            };

            let source = match event.source.as_ref() {
                Some(s) => s,
                None => continue,
            };

            let sender_id = source.user_id.as_deref().unwrap_or("unknown");
            let chat_id = match source.source_type.as_str() {
                "group" => source.group_id.as_deref().unwrap_or(sender_id),
                "room" => source.room_id.as_deref().unwrap_or(sender_id),
                _ => sender_id,
            };

            // Store reply token for response
            if let Some(ref reply_token) = event.reply_token {
                reply_tokens.insert(chat_id.to_string(), reply_token.clone());
            }

            let inbound = InboundMessage {
                channel: "line".to_string(),
                sender_id: sender_id.to_string(),
                chat_id: chat_id.to_string(),
                content: text.to_string(),
                media: Vec::new(),
                session_key: chat_id.to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
                voice_playback: None,
            };

            let _ = bus.send(inbound);
        }
    }
}

#[async_trait]
impl Channel for LineChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!("[LineChannel] starting LINE channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let channel_secret = self.config.channel_secret.clone();
        let webhook_path = self.config.webhook_path_resolved();
        let reply_tokens = self.reply_tokens.clone();
        let running = self.running.clone();
        let port = if self.config.webhook_port == 0 {
            8080
        } else {
            self.config.webhook_port
        };
        let host = self.config.webhook_host_resolved().to_string();

        // bind 在 spawn **之前**完成并把失败传播给调用方（wechat/wecom 同款）——
        // 此前 bind 失败只在后台任务里 warn，start() 照常报成功，入站能力
        // 归零而状态面显示运行中。bind 主机来自 config（空值回落
        // "0.0.0.0" 旧行为）——反代/隧道部署可配 "127.0.0.1" 只听回环。
        let listener = tokio::net::TcpListener::bind((host.as_str(), port))
            .await
            .map_err(|e| {
                NemesisError::Channel(format!(
                    "[LineChannel] webhook bind failed on {host}:{port}: {e}"
                ))
            })?;

        tokio::spawn(async move {
            info!("[LineChannel] webhook server listening on {host}:{port}");

            loop {
                if !*running.read() {
                    break;
                }

                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("[LineChannel] webhook accept error: {e}");
                        continue;
                    }
                };

                let bus = bus.clone();
                let secret = channel_secret.clone();
                let webhook_path = webhook_path.clone();
                let reply_tokens = reply_tokens.clone();
                let running = running.clone();

                tokio::spawn(async move {
                    Self::handle_webhook_connection(
                        stream,
                        &bus,
                        &secret,
                        &webhook_path,
                        &reply_tokens,
                        &running,
                    )
                    .await;
                });
            }

            info!("[LineChannel] webhook server stopped");
        });

        info!("[LineChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[LineChannel] stopping LINE channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        self.reply_tokens.clear();
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "LINE channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        // LINE 硬限 5000 字符（服务端 400 拒收整条），预算
        // 4900 留余量。reply token 一次性——首块走 reply()，其余块走 push。
        let chunks = split_message(&msg.content, 4900);

        // Prefer reply token if available
        if let Some((_, reply_token)) = self.reply_tokens.remove(&msg.chat_id) {
            self.reply(&reply_token, &chunks[0]).await?;
            for chunk in &chunks[1..] {
                self.push_message(&msg.chat_id, chunk).await?;
            }
            return Ok(());
        }

        // Fall back to push message
        for chunk in &chunks {
            self.push_message(&msg.chat_id, chunk).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

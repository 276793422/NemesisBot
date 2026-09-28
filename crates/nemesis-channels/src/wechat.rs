//! 微信个人微信通道（P29，Wave 3）：iLink Bot API（合规路径）。
//!
//! ⚠️ **协议形态是实现假设（D-3 决策点 fallback 形态）**：实施时无法网络探测
//! iLink Bot API 的真实申请门槛与协议细节，本模块基于公开资料按「HTTPS
//! webhook/回调 + token 鉴权」形态实现。**真机端到端未做（挂账）**；真机接入
//! 时以 iLink 官方文档校准，理想情况下只调 config（base_url / token / 回调
//! 路径 / 签名方案），代码面校准点集中在四处：
//!
//! 1. [`SignatureScheme`]——签名算法族（三态可配置，可插拔）；
//! 2. [`compute_hmac_sha256_signature`]——HMAC 输入拼接格式（当前假设
//!    `{timestamp}\n{nonce}\n{body}`）；
//! 3. [`WeChatCallbackMessage`]——入站消息 wire schema（serde alias 已做一轮
//!    宽容命名）；
//! 4. [`SendMessageRequest`]——出站请求 wire schema。
//!
//! 形态参照：`line.rs`（HTTP 回调端点 + 验签 + 自持监听）、`feishu.rs` /
//! `dingtalk.rs`（消息映射与 InboundMessage 组装）、`webhook_inbound.rs`。
//!
//! 入站：自持 HTTP server——`GET {callback_path}` URL 验证握手（经典微信形态，
//! 回显 echostr）+ `POST {callback_path}` 消息回调（验签后发布 InboundMessage）。
//! 出站：REST `POST {base_url}{send_path}`，`Authorization: Bearer {token}`。
//! token 只进 config 结构，绝不硬编码。
//!
//! 已知边界（诚实声明）：
//! - 停机语义同 line.rs：`stop()` 置 running=false 后，监听 socket 在下一个
//!   连接到来后才释放（accept 阻塞语义）；测试用随机端口规避占用。
//! - 单次 read 上限 64KB：超长请求体截断 → 400（文本消息场景足够）。
//! - 注入检测/凭据扫描等安全层在 gateway 侧 8 层管线，通道层不重复判断。
//! - HMAC 签名是**防伪造**而非防重放：timestamp/nonce 参与摘要但无时效窗、
//!   无 nonce 去重——截获的合法回调可无限重放（上公网前必须补齐时效窗/
//!   nonce 缓存，或由前置网关层防重放；见 [`SignatureScheme`] 文档）。

#![allow(dead_code)] // 通道 API client——schema 为实现假设，字段留白待真机校准

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

type HmacSha256 = Hmac<Sha256>;

/// POST 回调签名 HTTP 头（大小写不敏感；缺省回落 query 的 `signature` 参数）。
pub const SIGNATURE_HEADER: &str = "x-wechat-signature";

/// 回调监听默认地址（假设值，config `callback_listen_addr` 可覆盖）。
pub const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:9541";
/// 回调路径默认值（假设值，config `callback_path` 可覆盖）。
pub const DEFAULT_CALLBACK_PATH: &str = "/wechat/callback";
/// 出站消息接口默认路径（假设值，config `send_path` 可覆盖）。
pub const DEFAULT_SEND_PATH: &str = "/v1/message/send";
/// 出站 REST 默认基址（**假设值**——iLink 真实网关地址待真机校准）。
pub const DEFAULT_BASE_URL: &str = "https://ilink.bot.weixin.qq.com";

// ---------------------------------------------------------------------------
// 签名方案（可插拔——iLink 真实形态待真机校准，config 一键切换）
// ---------------------------------------------------------------------------

/// 回调签名方案（config `signature_scheme` 字符串解析）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignatureScheme {
    /// HMAC-SHA256(key=token, msg=`{timestamp}\n{nonce}\n{body}`) hex 小写
    /// （默认假设——现代 webhook 常见形态，防**伪造**）。⚠️ 当前实现只校验
    /// 签名本身（timestamp/nonce 参与摘要但不做时效窗/去重）——截获的合法
    /// 回调可被原样重放（签名恒有效）。重放防护（时效窗 + nonce LRU）挂账
    /// 待做；上公网前必须补齐或由网关层防重放。
    #[default]
    HmacSha256,
    /// 微信经典形态：`hex_sha1(sort([token, timestamp, nonce]).join(""))`。
    Sha1,
    /// 显式关闭验签（仅限本地开发调试；start 时 loud warn）。
    None,
}

impl SignatureScheme {
    /// 从 config 字符串解析（大小写不敏感；空值 = 默认 hmac-sha256；
    /// 未知值返回 None 由调用方 loud 拒绝，不静默降级）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "hmac-sha256" | "hmac_sha256" | "hmacsha256" => Some(Self::HmacSha256),
            "sha1" => Some(Self::Sha1),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// 方案名的规范字符串形态（日志/诊断用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac-sha256",
            Self::Sha1 => "sha1",
            Self::None => "none",
        }
    }
}

/// 计算微信经典 SHA1 签名：`sha1(sorted([token, timestamp, nonce]).join(""))`。
pub fn compute_sha1_signature(token: &str, timestamp: &str, nonce: &str) -> String {
    let mut parts = [token, timestamp, nonce];
    parts.sort();
    let mut hasher = Sha1::new();
    hasher.update(parts.join(""));
    hex::encode(hasher.finalize())
}

/// 计算 HMAC-SHA256 签名：`HMAC(key=token, msg={timestamp}\n{nonce}\n{body})`
/// hex 小写。**校准点**：若 iLink 真实协议的 MAC 输入拼接格式不同，只改这里。
pub fn compute_hmac_sha256_signature(
    token: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
) -> String {
    let mut mac =
        HmacSha256::new_from_slice(token.as_bytes()).expect("HMAC-SHA256 accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(b"\n");
    mac.update(nonce.as_bytes());
    mac.update(b"\n");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// 常量时间字节比较（防时序侧信道）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 统一验签入口（scheme 可插拔）。
///
/// - `HmacSha256`：`provided` 与 [`compute_hmac_sha256_signature`] 结果比较；
/// - `Sha1`：`provided` 与 [`compute_sha1_signature`] 结果比较（body 不参与，
///   经典微信形态）；
/// - `None`：全部放行（显式开发调试）；
/// - timestamp / nonce 缺失 → 拒绝（None 除外）。
pub fn verify_callback_signature(
    scheme: SignatureScheme,
    token: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
    provided: &str,
) -> bool {
    if scheme == SignatureScheme::None {
        return true;
    }
    if timestamp.is_empty() || nonce.is_empty() {
        return false;
    }
    let expected = match scheme {
        SignatureScheme::HmacSha256 => compute_hmac_sha256_signature(token, timestamp, nonce, body),
        SignatureScheme::Sha1 => compute_sha1_signature(token, timestamp, nonce),
        SignatureScheme::None => unreachable!("None scheme early-returned above"),
    };
    // hex 大小写归一（平台侧签名可能为大写 hex），再常量时间比较
    constant_time_eq(
        expected.to_ascii_lowercase().as_bytes(),
        provided.trim().to_ascii_lowercase().as_bytes(),
    )
}

// ---------------------------------------------------------------------------
// Config（全部 config 化——真机接入只调这里）
// ---------------------------------------------------------------------------

/// 微信个人微信通道配置（iLink Bot API）。
#[derive(Debug, Clone)]
pub struct WeChatConfig {
    /// 出站 REST 基址（默认 [`DEFAULT_BASE_URL`]，假设值）。
    pub base_url: String,
    /// 回调鉴权 token（只进 config，绝不硬编码）。
    pub token: String,
    /// 出站消息接口路径（默认 [`DEFAULT_SEND_PATH`]，假设值）。
    pub send_path: String,
    /// 回调监听地址（默认 [`DEFAULT_LISTEN_ADDR`]）。
    pub callback_listen_addr: String,
    /// 回调路径（默认 [`DEFAULT_CALLBACK_PATH`]）。
    pub callback_path: String,
    /// 签名方案：`hmac-sha256`（默认）/ `sha1` / `none`。
    pub signature_scheme: String,
    /// 允许的发送者 ID（空 = 不限；被过滤的消息仍 ack 200 防平台重试）。
    pub allow_from: Vec<String>,
    /// 联系人映射：user_id → 备注别名（命中时写入 metadata `contact_alias`）。
    pub contacts: HashMap<String, String>,
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            token: String::new(),
            send_path: DEFAULT_SEND_PATH.to_string(),
            callback_listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            callback_path: DEFAULT_CALLBACK_PATH.to_string(),
            signature_scheme: "hmac-sha256".to_string(),
            allow_from: Vec::new(),
            contacts: HashMap::new(),
        }
    }
}

impl WeChatConfig {
    /// 解析签名方案；未知值 loud 报错（不给静默降级）。
    pub fn signature_scheme(&self) -> Result<SignatureScheme> {
        SignatureScheme::parse(&self.signature_scheme).ok_or_else(|| {
            NemesisError::Channel(format!(
                "wechat 未知签名方案 {:?}（支持 hmac-sha256 / sha1 / none）",
                self.signature_scheme
            ))
        })
    }

    /// 回调监听地址（空值回落默认）。
    pub fn callback_listen_addr_resolved(&self) -> String {
        if self.callback_listen_addr.trim().is_empty() {
            DEFAULT_LISTEN_ADDR.to_string()
        } else {
            self.callback_listen_addr.trim().to_string()
        }
    }

    /// 回调路径（空值回落默认；保证以 `/` 开头）。
    pub fn callback_path_resolved(&self) -> String {
        let p = self.callback_path.trim();
        if p.is_empty() {
            DEFAULT_CALLBACK_PATH.to_string()
        } else if p.starts_with('/') {
            p.to_string()
        } else {
            format!("/{p}")
        }
    }

    /// 出站完整 URL：`{base_url}{send_path}`（空 base_url 返回 None，
    /// 调用方报错——出站依赖真实网关地址，不做假设兜底）。
    pub fn send_url(&self) -> Option<String> {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return None;
        }
        let path = self.send_path.trim();
        let path = if path.is_empty() {
            DEFAULT_SEND_PATH.to_string()
        } else if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Some(format!("{base}{path}"))
    }
}

// ---------------------------------------------------------------------------
// Wire schema（⚠️ 实现假设，真机校准点）
// ---------------------------------------------------------------------------

/// 入站回调消息 wire schema（假设形态；serde alias 做了一轮宽容命名——
/// 真机 schema 不匹配时这里是 serde 校准点）。
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct WeChatCallbackMessage {
    /// 消息类型（`text` 及其他；非 text 仍取 content 透传）。
    #[serde(rename = "msg_type", alias = "msgtype")]
    pub msg_type: String,
    /// 发送者用户 ID。
    #[serde(rename = "from_user_id", alias = "sender_id")]
    pub from_user_id: String,
    /// 发送者昵称（可选）。
    #[serde(rename = "from_nickname", alias = "sender_nick")]
    pub from_nickname: String,
    /// 会话 ID（群聊 = 群 ID；缺省回落 from_user_id 单聊）。
    #[serde(alias = "conversation_id")]
    pub chat_id: String,
    /// 会话类型：single / group（可选，进 metadata）。
    #[serde(rename = "chat_type", alias = "conversation_type")]
    pub chat_type: String,
    /// 文本内容。
    pub content: String,
    /// 消息 ID（可选，进 metadata）。
    #[serde(rename = "msg_id", alias = "message_id")]
    pub msg_id: String,
}

impl Default for WeChatCallbackMessage {
    fn default() -> Self {
        Self {
            msg_type: String::new(),
            from_user_id: String::new(),
            from_nickname: String::new(),
            chat_id: String::new(),
            chat_type: String::new(),
            content: String::new(),
            msg_id: String::new(),
        }
    }
}

/// 出站发送消息请求体（假设形态；真机 schema 不匹配时这里是校准点）。
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    to_user_id: String,
    msg_type: String,
    content: String,
}

// ---------------------------------------------------------------------------
// 回调处理
// ---------------------------------------------------------------------------

/// 回调请求处理结果（HTTP 响应三态以上的枚举，纯逻辑可测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackReply {
    /// 200 + 指定 Content-Type + body（握手回 echostr；消息回调回 ack JSON）。
    Ok {
        content_type: &'static str,
        body: String,
    },
    /// 400 Bad Request（缺参数 / body 解析失败——平台可重试）。
    BadRequest,
    /// 403 Forbidden（验签失败 / 缺签名）。
    Forbidden,
    /// 404 Not Found（回调路径不匹配）。
    NotFound,
    /// 405 Method Not Allowed（GET/POST 之外）。
    MethodNotAllowed,
}

impl CallbackReply {
    /// 200 文本回复（URL 验证握手回显 echostr 用）。
    fn ok_text(body: String) -> Self {
        Self::Ok {
            content_type: "text/plain; charset=utf-8",
            body,
        }
    }

    /// 200 JSON 回复（消息回调 ack 用）。
    fn ok_json(body: String) -> Self {
        Self::Ok {
            content_type: "application/json",
            body,
        }
    }

    /// HTTP 状态码（测试/诊断用）。
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Ok { .. } => 200,
            Self::BadRequest => 400,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
        }
    }

    /// 渲染为完整 HTTP 响应（Connection: close，单请求单连接语义）。
    pub fn to_http_response(&self) -> String {
        match self {
            Self::Ok { content_type, body } => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
            other => format!(
                "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                other.status_code(),
                match other {
                    Self::BadRequest => "Bad Request",
                    Self::Forbidden => "Forbidden",
                    Self::NotFound => "Not Found",
                    Self::MethodNotAllowed => "Method Not Allowed",
                    Self::Ok { .. } => unreachable!("Ok handled above"),
                }
            ),
        }
    }
}

/// 最小 percent-decode（query 值解码；`+` 视作空格，非法序列原样保留）。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex_part = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex_part, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 解析 query string 为 (key, decoded_value) 列表（容忍缺值）。
pub fn parse_query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// 取 query 参数值。
fn query_get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

/// 微信个人微信通道（iLink Bot API，回调型）。
pub struct WeChatChannel {
    base: BaseChannel,
    config: WeChatConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl WeChatChannel {
    /// 创建通道；token 必填、签名方案未知值 loud 拒绝（fail-fast）。
    pub fn new(
        config: WeChatConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.token.is_empty() {
            return Err(NemesisError::Channel(
                "wechat token is required（回调鉴权 token，只进 config）".to_string(),
            ));
        }
        // 签名方案在装配期校验，坏 config 不给带病启动
        config.signature_scheme()?;

        // allow_from 进 BaseChannel 白名单（复合 ID 匹配语义与记账同 base.handle_message）
        Ok(Self {
            base: BaseChannel::with_allow_list("wechat", config.allow_from.clone()),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            bus_sender,
        })
    }

    /// 解析并发布入站消息（纯逻辑，HTTP 层薄封装）。
    ///
    /// 返回 `true` = 请求体被接受（含 allow-list 过滤——过滤也 ack 200，防平台
    /// 无限重试）；`false` = 解析失败 / 空内容（调用方回 4xx 让平台重试）。
    fn publish_inbound(
        config: &WeChatConfig,
        bus_sender: &broadcast::Sender<InboundMessage>,
        base: &BaseChannel,
        body: &[u8],
    ) -> bool {
        let msg: WeChatCallbackMessage = match serde_json::from_slice(body) {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "[WeChatChannel] 回调 body 解析失败（wire schema 待真机校准点）");
                return false;
            }
        };

        if msg.content.is_empty() {
            debug!("[WeChatChannel] 回调消息 content 为空，忽略");
            return false;
        }

        let sender_id = if msg.from_user_id.is_empty() {
            "unknown"
        } else {
            msg.from_user_id.as_str()
        };

        // allow-list 过滤（base.handle_message 同步记账 received）；被过滤仍算
        // 「接受」——上层 ack 200，平台不再重试。
        if !base.handle_message(sender_id) {
            debug!(sender_id = %sender_id, "[WeChatChannel] 消息被 allow_from 过滤");
            return true;
        }

        let chat_id = if msg.chat_id.is_empty() {
            sender_id.to_string()
        } else {
            msg.chat_id.clone()
        };

        let mut metadata = HashMap::new();
        if !msg.msg_type.is_empty() {
            metadata.insert("msg_type".to_string(), msg.msg_type.clone());
        }
        if !msg.chat_type.is_empty() {
            metadata.insert("chat_type".to_string(), msg.chat_type.clone());
        }
        if !msg.msg_id.is_empty() {
            metadata.insert("msg_id".to_string(), msg.msg_id.clone());
        }
        if !msg.from_nickname.is_empty() {
            metadata.insert("from_nickname".to_string(), msg.from_nickname.clone());
        }
        // 联系人映射：命中写入备注别名
        if let Some(alias) = config.contacts.get(sender_id) {
            metadata.insert("contact_alias".to_string(), alias.clone());
        }

        let inbound = InboundMessage {
            channel: "wechat".to_string(),
            sender_id: sender_id.to_string(),
            chat_id: chat_id.clone(),
            content: msg.content,
            media: Vec::new(),
            session_key: format!("wechat:{chat_id}"),
            correlation_id: String::new(),
            metadata,
            voice_playback: None,
        };

        info!(
            sender_id = %inbound.sender_id,
            chat_id = %inbound.chat_id,
            "[WeChatChannel] received message"
        );

        if let Err(e) = bus_sender.send(inbound) {
            warn!("[WeChatChannel] failed to publish inbound message: {e}");
        }
        true
    }

    /// 处理一笔回调请求（纯逻辑入口：method / path / query / body / 签名头）。
    pub fn handle_callback_with(
        config: &WeChatConfig,
        bus_sender: &broadcast::Sender<InboundMessage>,
        base: &BaseChannel,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
        signature_header: Option<&str>,
    ) -> CallbackReply {
        if path != config.callback_path_resolved() {
            return CallbackReply::NotFound;
        }

        // 签名方案装配期已校验；此处防御性兜底（直构 config 的测试路径）
        let scheme = match config.signature_scheme() {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "[WeChatChannel] 签名方案配置非法");
                return CallbackReply::BadRequest;
            }
        };

        let params = parse_query_params(query);
        let timestamp = query_get(&params, "timestamp").unwrap_or("").to_string();
        let nonce = query_get(&params, "nonce").unwrap_or("").to_string();
        let provided = signature_header
            .map(str::to_string)
            .or_else(|| query_get(&params, "signature").map(str::to_string));
        let provided = match provided {
            Some(p) if !p.trim().is_empty() => p,
            _ => {
                warn!("[WeChatChannel] 回调缺签名（header 与 query signature 均为空）");
                return CallbackReply::Forbidden;
            }
        };

        match method.to_ascii_uppercase().as_str() {
            "GET" => {
                // URL 验证握手（经典微信形态）：验签通过原样回显 echostr
                let echostr = query_get(&params, "echostr").unwrap_or("");
                if echostr.is_empty() {
                    return CallbackReply::BadRequest;
                }
                if verify_callback_signature(
                    scheme,
                    &config.token,
                    &timestamp,
                    &nonce,
                    b"",
                    &provided,
                ) {
                    CallbackReply::ok_text(echostr.to_string())
                } else {
                    warn!("[WeChatChannel] URL 验证握手验签失败");
                    CallbackReply::Forbidden
                }
            }
            "POST" => {
                if !verify_callback_signature(
                    scheme,
                    &config.token,
                    &timestamp,
                    &nonce,
                    body,
                    &provided,
                ) {
                    warn!("[WeChatChannel] 消息回调验签失败");
                    return CallbackReply::Forbidden;
                }
                if Self::publish_inbound(config, bus_sender, base, body) {
                    CallbackReply::ok_json("{\"code\":0}".to_string())
                } else {
                    CallbackReply::BadRequest
                }
            }
            _ => CallbackReply::MethodNotAllowed,
        }
    }

    /// 处理单条回调 TCP 连接（最小 HTTP 解析，line.rs 同款形态）。
    async fn handle_connection(
        stream: tokio::net::TcpStream,
        config: &WeChatConfig,
        bus_sender: &broadcast::Sender<InboundMessage>,
        base: &BaseChannel,
        running: &parking_lot::RwLock<bool>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

        // 请求行：METHOD /path?query HTTP/1.x
        let request_line = request_str.lines().next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (target, ""),
        };

        // 签名头（大小写不敏感；只看 header 区，遇空行即止）
        let mut signature_header: Option<&str> = None;
        for line in request_str.lines().skip(1) {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case(SIGNATURE_HEADER) {
                    signature_header = Some(value.trim());
                }
            }
        }

        // body = \r\n\r\n 之后的字节（按原始字节定位，非 lossy 字符串）
        let body = match request_data.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(idx) => &request_data[idx + 4..],
            None => &[],
        };

        let reply = Self::handle_callback_with(
            config,
            bus_sender,
            base,
            method,
            path,
            query,
            body,
            signature_header,
        );
        let _ = stream.write_all(reply.to_http_response().as_bytes()).await;
    }

    /// 自持回调 HTTP server（accept 循环，line.rs 同款停机语义）。
    ///
    /// bind 在 spawn **之前**完成并把失败传播给调用方（wecom 同款）——
    /// 此前 bind 失败只在后台任务里 warn，start() 照常报成功，入站能力
    /// 归零而状态面显示运行中。
    async fn spawn_callback_server(&self) -> Result<()> {
        let bus_sender = self.bus_sender.clone();
        let config = self.config.clone();
        let base = self.base.clone();
        let running = self.running.clone();
        let listen_addr = self.config.callback_listen_addr_resolved();

        let listener = tokio::net::TcpListener::bind(&listen_addr).await.map_err(|e| {
            NemesisError::Channel(format!(
                "[WeChatChannel] 回调监听绑定失败 {listen_addr}: {e}"
            ))
        })?;

        tokio::spawn(async move {
            info!("[WeChatChannel] callback server listening on {listen_addr}");

            loop {
                if !*running.read() {
                    break;
                }

                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(error = %e, "[WeChatChannel] callback accept error");
                        continue;
                    }
                };

                let bus_sender = bus_sender.clone();
                let config = config.clone();
                let base = base.clone();
                let running = running.clone();

                tokio::spawn(async move {
                    Self::handle_connection(stream, &config, &bus_sender, &base, &running).await;
                });
            }

            info!("[WeChatChannel] callback server stopped");
        });
        Ok(())
    }

    /// 出站：`POST {base_url}{send_path}`，Bearer token 鉴权，JSON 体
    /// `{to_user_id, msg_type:"text", content}`（⚠️ 假设 schema，真机校准点）。
    pub async fn send_text(&self, to_user_id: &str, text: &str) -> Result<()> {
        let url = self
            .config
            .send_url()
            .ok_or_else(|| NemesisError::Channel("wechat base_url 未配置（config）".to_string()))?;

        let request = SendMessageRequest {
            to_user_id: to_user_id.to_string(),
            msg_type: "text".to_string(),
            content: text.to_string(),
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("wechat send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!("wechat send error: {body}")));
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for WeChatChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        let scheme = self.config.signature_scheme()?;
        info!(
            listen = %self.config.callback_listen_addr_resolved(),
            callback_path = %self.config.callback_path_resolved(),
            scheme = scheme.as_str(),
            "[WeChatChannel] starting wechat channel (iLink Bot API，协议形态为实现假设，真机待校准)"
        );
        if scheme == SignatureScheme::None {
            warn!(
                "[WeChatChannel] 签名校验已显式关闭（signature_scheme=none）——仅限本地开发调试，勿用于公网"
            );
        }

        *self.running.write() = true;
        self.base.set_enabled(true);
        self.base.set_running(true);

        self.spawn_callback_server().await?;

        info!("[WeChatChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[WeChatChannel] stopping wechat channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        self.base.set_running(false);
        // 监听 socket 停机语义同 line.rs：accept 阻塞，下一个连接到来后退出
        info!("[WeChatChannel] channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "wechat channel not running".to_string(),
            ));
        }

        if msg.chat_id.is_empty() {
            return Err(NemesisError::Channel("chat ID is empty".to_string()));
        }

        self.base.record_sent();
        debug!(chat_id = %msg.chat_id, "[WeChatChannel] sending message");
        self.send_text(&msg.chat_id, &msg.content).await?;
        // 出站同步镜像（mqtt/websocket 先例：镜像到 web 等同步目标）
        self.base.sync_to_targets(&msg.content).await;
        Ok(())
    }

    fn add_sync_target(&self, name: &str, channel: Arc<dyn Channel>) -> Result<()> {
        self.base.add_sync_target(name, channel)
    }

    fn remove_sync_target(&self, name: &str) {
        self.base.remove_sync_target(name);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

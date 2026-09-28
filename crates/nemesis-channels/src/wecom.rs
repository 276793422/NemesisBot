//! WeCom（企业微信）通道（P25，Wave 3）。
//!
//! **出站**：群机器人 webhook POST——群机器人 webhook 是企业微信群内
//! 「添加机器人」生成的 HTTPS 地址，POST JSON（`msgtype: text|markdown`），
//! 响应 `{"errcode":0,"errmsg":"ok"}`，协议与钉钉自定义机器人 webhook 同族。
//!
//! **入站**：智能机器人回调——自托管 HTTP 端点（axum），走企业微信回调
//! 消息加解密协议（`wecom::crypto`）：GET 验证 URL（解密 echostr 回明文）+
//! POST 收消息（SHA1 签名校验 + AES-256-CBC 解密）→ 发 bus InboundMessage。
//!
//! 装配模式参照 feishu/dingtalk：`Channel` trait + `BaseChannel` 复用，
//! config 由 `ChannelInitConfig.wecom` 注入（feature `wecom` 门控）。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

pub mod crypto;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// WeCom 通道配置。
///
/// 两种工作形态可独立启用：
/// - 出站：`webhook_url`（群机器人 webhook），可选 `webhooks` 多群路由；
/// - 入站：`token` + `encoding_aes_key` 齐备时自托管回调端点。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WeComConfig {
    /// 群机器人 webhook 地址（出站默认目标）。
    #[serde(default)]
    pub webhook_url: String,
    /// chat_id → 群机器人 webhook 路由表（多群出站；chat_id 命中优先于默认）。
    #[serde(default)]
    pub webhooks: HashMap<String, String>,
    /// 回调验签 token（企业微信后台配置的 Token）。
    #[serde(default)]
    pub token: String,
    /// 回调消息加密密钥 EncodingAESKey（43 字符；私钥字段，只进 config）。
    #[serde(default)]
    pub encoding_aes_key: String,
    /// 企业微信 corp_id（解密 receiveid 强校验用；空 = 不校验，
    /// 智能机器人回调 receiveid 可能为空串）。
    #[serde(default)]
    pub corp_id: String,
    /// 回调 HTTP 监听地址（默认 0.0.0.0:9898）。
    #[serde(default)]
    pub listen_addr: String,
    /// 回调路径（默认 /wecom/callback）。
    #[serde(default)]
    pub callback_path: String,
    /// 允许的发送者 ID（空 = 不限）。
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// 回调监听地址默认值。
const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:9898";
/// 回调路径默认值。
const DEFAULT_CALLBACK_PATH: &str = "/wecom/callback";

// ---------------------------------------------------------------------------
// 通道实现
// ---------------------------------------------------------------------------

/// WeCom 通道：群机器人 webhook（出站）+ 智能机器人回调（入站）。
pub struct WeComChannel {
    base: BaseChannel,
    config: WeComConfig,
    http: reqwest::Client,
    /// 从 EncodingAESKey 派生的 AES 密钥（入站回调启用时为 Some）。
    aes_key: Option<[u8; 32]>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// 回调服务优雅停机句柄。
    shutdown_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// 回调服务实际绑定地址（start 后有值；测试用它拿随机端口）。
    callback_addr: parking_lot::RwLock<Option<std::net::SocketAddr>>,
}

impl WeComChannel {
    /// Creates a new `WeComChannel`.
    pub fn new(config: WeComConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        // 至少要配一侧：出站 webhook 或入站回调密钥，否则通道没有存在意义
        if config.webhook_url.is_empty()
            && config.webhooks.is_empty()
            && config.encoding_aes_key.is_empty()
        {
            return Err(NemesisError::Channel(
                "wecom: webhook_url（出站）与 encoding_aes_key（入站回调）至少配置一项".to_string(),
            ));
        }

        // EncodingAESKey 配了就必须合法（fail-fast 到 new，不等 start）
        let aes_key = if config.encoding_aes_key.is_empty() {
            None
        } else {
            Some(crypto::aes_key_from_encoding(&config.encoding_aes_key)?)
        };

        let base = if config.allow_from.is_empty() {
            BaseChannel::new("wecom")
        } else {
            BaseChannel::with_allow_list("wecom", config.allow_from.clone())
        };

        // 缺省值归一（空串 → 协议默认）
        let mut config = config;
        if config.listen_addr.is_empty() {
            config.listen_addr = DEFAULT_LISTEN_ADDR.to_string();
        }
        if config.callback_path.is_empty() {
            config.callback_path = DEFAULT_CALLBACK_PATH.to_string();
        }

        Ok(Self {
            base,
            config,
            http: reqwest::Client::new(),
            aes_key,
            bus_sender,
            shutdown_tx: parking_lot::Mutex::new(None),
            callback_addr: parking_lot::RwLock::new(None),
        })
    }

    /// 返回回调服务实际绑定地址（start 前为 None）。
    pub fn callback_addr(&self) -> Option<std::net::SocketAddr> {
        *self.callback_addr.read()
    }

    /// 解析出站 webhook 目标地址。
    ///
    /// 优先级：chat_id 本身是完整 URL > `webhooks` 路由表 > 默认 `webhook_url`。
    fn resolve_webhook(&self, msg: &OutboundMessage) -> Result<String> {
        if msg.chat_id.starts_with("http://") || msg.chat_id.starts_with("https://") {
            return Ok(msg.chat_id.clone());
        }
        if let Some(url) = self.config.webhooks.get(&msg.chat_id) {
            return Ok(url.clone());
        }
        if !self.config.webhook_url.is_empty() {
            return Ok(self.config.webhook_url.clone());
        }
        Err(NemesisError::Channel(format!(
            "wecom: chat_id '{}' 无对应 webhook（webhooks 路由表与默认 webhook_url 均未命中）",
            msg.chat_id
        )))
    }

    /// 构造群机器人 webhook 请求体。
    ///
    /// 消息格式映射：`message_type == "text"` → text，其余（含空）→ markdown
    /// （markdown 表达力更强，作默认，与钉钉通道同取向）。
    fn build_webhook_body(msg: &OutboundMessage) -> serde_json::Value {
        if msg.message_type.eq_ignore_ascii_case("text") {
            serde_json::json!({
                "msgtype": "text",
                "text": { "content": msg.content },
            })
        } else {
            serde_json::json!({
                "msgtype": "markdown",
                "markdown": { "content": msg.content },
            })
        }
    }

    /// 通过群机器人 webhook 发送一条消息（HTTP POST + errcode 检查）。
    pub async fn send_webhook(&self, webhook_url: &str, msg: &OutboundMessage) -> Result<()> {
        let body = Self::build_webhook_body(msg);

        let resp = self
            .http
            .post(webhook_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("wecom webhook 请求失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "wecom webhook HTTP {status}: {text}"
            )));
        }

        // 企业微信业务错误走 errcode != 0（HTTP 仍 200）
        let payload: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("wecom webhook 响应解析失败: {e}")))?;
        let errcode = payload.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
        if errcode != 0 {
            let errmsg = payload.get("errmsg").and_then(|v| v.as_str()).unwrap_or("");
            return Err(NemesisError::Channel(format!(
                "wecom webhook errcode={errcode}: {errmsg}"
            )));
        }

        Ok(())
    }

    /// 拉起智能机器人回调 HTTP 服务（axum，自托管端口）。
    async fn spawn_callback_server(&self) -> Result<()> {
        // 重复 start 先优雅停掉旧实例
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }

        let listener = tokio::net::TcpListener::bind(&self.config.listen_addr)
            .await
            .map_err(|e| {
                NemesisError::Channel(format!(
                    "wecom: 回调端口绑定 {} 失败: {e}",
                    self.config.listen_addr
                ))
            })?;
        let local = listener
            .local_addr()
            .map_err(|e| NemesisError::Channel(format!("wecom: 获取回调监听地址失败: {e}")))?;

        // BaseChannel::clone 共享 stats Arc 与 allow_list 快照——回调任务里
        // 记账/allow 过滤与通道本体同源
        let state = Arc::new(CallbackState {
            token: self.config.token.clone(),
            aes_key: self.aes_key.expect("inbound 已由 token+aes_key 齐备性保证"),
            corp_id: self.config.corp_id.clone(),
            base: self.base.clone(),
            bus_sender: self.bus_sender.clone(),
        });

        let app = axum::Router::new()
            .route(
                &self.config.callback_path,
                axum::routing::get(callback_verify).post(callback_message),
            )
            .with_state(state);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock() = Some(shutdown_tx);

        tokio::spawn(async move {
            let shutdown = async move {
                let _ = shutdown_rx.await;
            };
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await
            {
                warn!(error = %e, "[WeComChannel] 回调服务异常退出");
            }
        });

        *self.callback_addr.write() = Some(local);
        info!(
            listen = %local,
            path = %self.config.callback_path,
            "[WeComChannel] 智能机器人回调服务已启动"
        );
        Ok(())
    }
}

#[async_trait]
impl Channel for WeComChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!("[WeComChannel] starting WeCom channel");

        // 入站回调：token + aes_key 齐备才开端口；仅出站模式不占端口
        let inbound_enabled =
            !self.config.token.is_empty() && !self.config.encoding_aes_key.is_empty();
        if inbound_enabled {
            self.spawn_callback_server().await?;
        } else {
            warn!(
                "[WeComChannel] 未配置 token/encoding_aes_key（二者需齐备），降级为仅出站 webhook 模式（不监听回调端口，入站能力关闭）"
            );
        }

        self.base.set_running(true);
        self.base.set_enabled(true);
        info!("[WeComChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[WeComChannel] stopping WeCom channel");

        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }
        *self.callback_addr.write() = None;

        self.base.set_running(false);
        self.base.set_enabled(false);
        info!("[WeComChannel] channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.base.is_running() {
            return Err(NemesisError::Channel(
                "wecom channel not running".to_string(),
            ));
        }

        let webhook = self.resolve_webhook(&msg)?;
        debug!(chat_id = %msg.chat_id, "[WeComChannel] sending message via webhook");
        self.base.record_sent();
        self.send_webhook(&webhook, &msg).await?;
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
// 回调 HTTP 端点
// ---------------------------------------------------------------------------

/// 回调服务共享状态（axum State）。
struct CallbackState {
    token: String,
    aes_key: [u8; 32],
    corp_id: String,
    base: BaseChannel,
    bus_sender: broadcast::Sender<InboundMessage>,
}

/// 从查询串取参数（缺省空串）。
fn qp<'a>(q: &'a HashMap<String, String>, key: &str) -> &'a str {
    q.get(key).map(|s| s.as_str()).unwrap_or("")
}

/// GET 验证 URL：企业微信后台保存回调配置时发起。
///
/// 校验 `msg_signature` → 解密 `echostr` → **明文原样**返回（企业微信按
/// 比对解密结果确认端点归属，返回必须是纯文本明文）。
async fn callback_verify(
    axum::extract::State(st): axum::extract::State<Arc<CallbackState>>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let echostr = qp(&q, "echostr");
    if echostr.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "missing echostr".to_string(),
        );
    }

    let signature = qp(&q, "msg_signature");
    if !crypto::verify_signature(
        &st.token,
        qp(&q, "timestamp"),
        qp(&q, "nonce"),
        echostr,
        signature,
    ) {
        warn!("[WeComChannel] URL 验证签名校验失败");
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "signature mismatch".to_string(),
        );
    }

    match crypto::decrypt_message(&st.aes_key, echostr) {
        Ok((plaintext, _)) => (axum::http::StatusCode::OK, plaintext),
        Err(e) => {
            warn!(error = %e, "[WeComChannel] URL 验证 echostr 解密失败");
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("decrypt failed: {e}"),
            )
        }
    }
}

/// POST 收消息：智能机器人回调（JSON `{"encrypt": ...}`）或经典应用回调
/// （XML `<Encrypt>...</Encrypt>`）双形态。
///
/// 流程：提取密文 → 验签 → 解密 → receiveid 校验 → 容忍式 JSON 解析 →
/// allow 过滤 → 发 bus。业务侧回 `{"errcode":0,"errmsg":"ok"}`；
/// 验签/解密失败回 400（企业微信会按失败处理）。
async fn callback_message(
    axum::extract::State(st): axum::extract::State<Arc<CallbackState>>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    body: String,
) -> impl axum::response::IntoResponse {
    let ok = || {
        (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({"errcode": 0, "errmsg": "ok"})),
        )
    };
    let bad = |reason: &str| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"errcode": -1, "errmsg": reason})),
        )
    };

    let encrypt = match extract_encrypt_from_body(&body) {
        Some(e) => e,
        None => {
            warn!("[WeComChannel] 回调请求体缺少 encrypt 字段");
            return bad("missing encrypt");
        }
    };

    let signature = qp(&q, "msg_signature");
    if !crypto::verify_signature(
        &st.token,
        qp(&q, "timestamp"),
        qp(&q, "nonce"),
        &encrypt,
        signature,
    ) {
        warn!("[WeComChannel] 消息回调签名校验失败");
        return bad("signature mismatch");
    }

    let (plaintext, receiveid) = match crypto::decrypt_message(&st.aes_key, &encrypt) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "[WeComChannel] 消息回调解密失败");
            return bad("decrypt failed");
        }
    };

    // receiveid 强校验：仅当 corp_id 已配置且回调携带 receiveid（自建应用形态）
    if !st.corp_id.is_empty() && !receiveid.is_empty() && st.corp_id != receiveid {
        warn!(
            expected = %st.corp_id,
            actual = %receiveid,
            "[WeComChannel] receiveid 与 corp_id 不匹配，拒绝"
        );
        return bad("receiveid mismatch");
    }

    let Some(parsed) = parse_callback_message(&plaintext) else {
        // 能解密但解析不了 = 我方 schema 覆盖不足，重试无益：回 ok 只告警
        warn!(
            plaintext_len = plaintext.len(),
            "[WeComChannel] 回调明文 JSON 解析失败，忽略"
        );
        return ok();
    };

    if !st.base.is_allowed(&parsed.sender_id) {
        debug!(sender_id = %parsed.sender_id, "[WeComChannel] 消息被 allow_list 过滤");
        return ok();
    }
    if parsed.content.is_empty() {
        debug!("[WeComChannel] 空内容消息，忽略");
        return ok();
    }

    let chat_id = if parsed.chat_id.is_empty() {
        if parsed.sender_id.is_empty() {
            "unknown".to_string()
        } else {
            parsed.sender_id.clone()
        }
    } else {
        parsed.chat_id.clone()
    };

    let mut metadata = HashMap::new();
    if !parsed.msg_type.is_empty() {
        metadata.insert("msg_type".to_string(), parsed.msg_type);
    }
    if !parsed.sender_nick.is_empty() {
        metadata.insert("sender_nick".to_string(), parsed.sender_nick);
    }
    if !receiveid.is_empty() {
        metadata.insert("receiveid".to_string(), receiveid);
    }

    let inbound = InboundMessage {
        channel: "wecom".to_string(),
        sender_id: parsed.sender_id.clone(),
        chat_id: chat_id.clone(),
        content: parsed.content,
        media: Vec::new(),
        session_key: format!("wecom:{}", chat_id),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };

    st.base.record_received();
    info!(
        sender_id = %inbound.sender_id,
        chat_id = %inbound.chat_id,
        "[WeComChannel] received message"
    );
    if let Err(e) = st.bus_sender.send(inbound) {
        warn!("[WeComChannel] 发布入站消息失败: {e}");
    }

    ok()
}

/// 从 POST 请求体提取 `encrypt` 密文：JSON `{"encrypt": "..."}` 优先，
/// XML `<Encrypt><![CDATA[...]]></Encrypt>` 兜底（覆盖企业微信经典回调形态）。
fn extract_encrypt_from_body(body: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(e) = v.get("encrypt").and_then(|e| e.as_str()) {
            return Some(e.to_string());
        }
    }

    // XML 形态（不做完整 XML 解析，按标签定位足够）
    let tag_pos = body.find("<Encrypt")?;
    let after_open = body[tag_pos..].find('>')? + tag_pos + 1;
    let rest = &body[after_open..];
    let end = rest.find("</Encrypt>")?;
    let mut val = rest[..end].trim();
    if let Some(inner) = val.strip_prefix("<![CDATA[") {
        val = inner.strip_suffix("]]>").unwrap_or(inner).trim();
    }
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// 回调明文的容忍式解析结果。
#[derive(Debug, Clone, PartialEq)]
pub struct CallbackMessage {
    pub msg_type: String,
    pub content: String,
    pub sender_id: String,
    pub sender_nick: String,
    pub chat_id: String,
}

/// 按候选键路径从 JSON 取字符串字段。
fn jstr<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

/// 解析回调明文 JSON（容忍式：智能机器人新格式 camelCase 优先，
/// 兼容经典应用回调字段形态）。
///
/// 真实智能机器人回调 wire schema 未做真机验证（无凭证），这里按多候选
/// 字段名覆盖主流形态，未覆盖形态解析失败会走「回 ok + 告警」路径，
/// 不会误回 400 造成平台重试风暴。
pub fn parse_callback_message(plaintext: &str) -> Option<CallbackMessage> {
    let v: serde_json::Value = serde_json::from_str(plaintext).ok()?;

    let content = jstr(&v, &["text", "content"])
        .or_else(|| jstr(&v, &["content"]))
        .or_else(|| jstr(&v, &["Content"]))
        .unwrap_or("")
        .to_string();

    let sender_id = jstr(&v, &["from", "userId"])
        .or_else(|| jstr(&v, &["from", "userid"]))
        .or_else(|| jstr(&v, &["from_userid"]))
        .or_else(|| jstr(&v, &["FromUserName"]))
        .unwrap_or("")
        .to_string();

    let sender_nick = jstr(&v, &["from", "name"])
        .or_else(|| jstr(&v, &["from_name"]))
        .or_else(|| jstr(&v, &["SenderNick"]))
        .unwrap_or("")
        .to_string();

    let chat_id = jstr(&v, &["chatId"])
        .or_else(|| jstr(&v, &["chatid"]))
        .or_else(|| jstr(&v, &["ChatId"]))
        .unwrap_or("")
        .to_string();

    let msg_type = jstr(&v, &["msgtype"])
        .or_else(|| jstr(&v, &["MsgType"]))
        .unwrap_or("text")
        .to_string();

    Some(CallbackMessage {
        msg_type,
        content,
        sender_id,
        sender_nick,
        chat_id,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

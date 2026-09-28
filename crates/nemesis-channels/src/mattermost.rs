//! Mattermost 通道（P26，Wave 3 能力扩展）：REST + WebSocket 双通路直连。
//!
//! - **入站**：WebSocket 网关 `{base}/api/v4/websocket`（连接后首帧发
//!   `authentication_challenge` 鉴权，bot token 模式，无需 OAuth/app token），
//!   消费 `posted` 事件 → `InboundMessage`。
//! - **出站**：REST `POST {base}/api/v4/posts`（`channel_id` + markdown 正文，
//!   可选 `root_id` 线程回复）。
//! - 与 slack 通道同构（`slack.rs` 为参照物）：指数退避重连 / `allow_from`
//!   用户过滤 / chat_id 复合形态 `CHANNEL_ID/ROOT_ID`（对应 slack 的
//!   `CHANNEL/THREAD_TS`）/ 提及剥离。
//! - 消息格式：Mattermost markdown 与 bot 内部格式同为标准 markdown 方言，
//!   正文字段恒等透传（代码块/粗斜体/列表双端原生支持）；格式映射只做
//!   方言差异的必要部分——入站剥离 `@botname` 提及（见 `strip_bot_mention`）。
//! - config：`base_url` + `bot_token` + `allow_from`（用户白名单，空=全放行）
//!   + `channels`（监听频道 ID/名称映射过滤，空=全部），token 只进 config
//!   结构，绝不硬编码。

#![allow(dead_code)] // channel API client — 完整 schema 镜像自 Mattermost API，部分字段暂未消费

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
/// WS 鉴权 challenge 的序号（Mattermost 协议要求请求带递增 seq）。
const AUTH_SEQ: u64 = 1;

// ---------------------------------------------------------------------------
// Mattermost API 类型
// ---------------------------------------------------------------------------

/// Mattermost 通道配置。
#[derive(Debug, Clone)]
pub struct MattermostConfig {
    /// 服务器基地址（如 `https://mattermost.example.com`，无尾斜杠；http/https）。
    pub base_url: String,
    /// Bot token（Mattermost Bot 账号的 access token，或个人访问令牌）。
    pub bot_token: String,
    /// 允许的用户 ID / 用户名白名单（空 = 全放行）。
    pub allow_from: Vec<String>,
    /// 监听的频道 ID / 名称映射（空 = bot 所在全部频道）。
    pub channels: Vec<String>,
}

/// Mattermost `POST /api/v4/posts` 请求体。
#[derive(Debug, Serialize)]
struct CreatePostParams {
    channel_id: String,
    message: String,
    /// 线程回复的父帖 ID（空 = 新话题顶层帖）。
    #[serde(skip_serializing_if = "Option::is_none")]
    root_id: Option<String>,
}

/// WebSocket 单连接会话的共享上下文（由重连循环逐连接传入）。
#[derive(Clone)]
struct WsCtx {
    bot_token: String,
    bot_user_id: Arc<parking_lot::RwLock<String>>,
    bot_username: Arc<parking_lot::RwLock<String>>,
    allow_from: Vec<String>,
    listen_channels: Vec<String>,
    bus_sender: broadcast::Sender<InboundMessage>,
    /// 通道 running 标志（与 start_ws_loop 共享）：事件循环按粒度轮询，
    /// stop() 能即时打断存活中的 WS 会话——否则旧会话继续收发直到连接
    /// 自然断开，随后的 start() 再起第二条会话 → 双会话重复入站。
    running: Arc<parking_lot::RwLock<bool>>,
}

// ---------------------------------------------------------------------------
// MattermostChannel
// ---------------------------------------------------------------------------

/// Mattermost 通道：WebSocket 网关收 + REST API 发。
pub struct MattermostChannel {
    base: BaseChannel,
    config: MattermostConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    /// Bot 账号 user_id（`users/me` 校验后填充；用于过滤自己的消息回声）。
    bot_user_id: Arc<parking_lot::RwLock<String>>,
    /// Bot 账号 username（提及检测 `@username` 与入站提及剥离用）。
    bot_username: Arc<parking_lot::RwLock<String>>,
    /// Bus sender，用于发布入站消息。
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl MattermostChannel {
    /// 创建 `MattermostChannel`。
    pub fn new(
        config: MattermostConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.base_url.trim().is_empty() {
            return Err(NemesisError::Channel(
                "mattermost base_url is required".to_string(),
            ));
        }
        let trimmed = config.base_url.trim();
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return Err(NemesisError::Channel(format!(
                "mattermost base_url must start with http:// or https://, got: {trimmed}"
            )));
        }
        if config.bot_token.trim().is_empty() {
            return Err(NemesisError::Channel(
                "mattermost bot_token is required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("mattermost"),
            config,
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            bot_user_id: Arc::new(parking_lot::RwLock::new(String::new())),
            bot_username: Arc::new(parking_lot::RwLock::new(String::new())),
            bus_sender,
        })
    }

    /// 设置 bot user_id（`users/me` 校验后 / 测试注入）。
    pub fn set_bot_user_id(&self, id: String) {
        *self.bot_user_id.write() = id;
    }

    /// 设置 bot username（提及检测用）。
    pub fn set_bot_username(&self, name: String) {
        *self.bot_username.write() = name;
    }

    /// 返回 bot user_id。
    pub fn bot_user_id(&self) -> String {
        self.bot_user_id.read().clone()
    }

    /// 拼接 REST API 绝对地址（基地址去尾斜杠 + `/api/v4` 前缀）。
    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/api/v4{}",
            self.config.base_url.trim().trim_end_matches('/'),
            path
        )
    }

    /// 基地址 → WebSocket 网关地址（https→wss、http→ws）。
    pub fn ws_url_from_base(base_url: &str) -> String {
        let trimmed = base_url.trim().trim_end_matches('/');
        if let Some(rest) = trimmed.strip_prefix("https://") {
            format!("wss://{rest}/api/v4/websocket")
        } else if let Some(rest) = trimmed.strip_prefix("http://") {
            format!("ws://{rest}/api/v4/websocket")
        } else {
            format!("{trimmed}/api/v4/websocket")
        }
    }

    /// 构造 WS 鉴权 challenge 帧（连接后首帧必须发送）。
    pub fn build_auth_message(token: &str) -> String {
        serde_json::json!({
            "seq": AUTH_SEQ,
            "action": "authentication_challenge",
            "data": { "token": token },
        })
        .to_string()
    }

    /// 解析复合 chat_id：`CHANNEL_ID` 或 `CHANNEL_ID/ROOT_ID`（线程回复）。
    pub fn parse_chat_id(chat_id: &str) -> (&str, Option<&str>) {
        if let Some(idx) = chat_id.find('/') {
            (&chat_id[..idx], Some(&chat_id[idx + 1..]))
        } else {
            (chat_id, None)
        }
    }

    /// 剥离文本中对 bot 的 `@username` 提及（slack `strip_bot_mention` 同款做法）。
    ///
    /// 优先按「提及 + 尾随空格」整段剥离（避免句中残留双空格），
    /// 紧贴标点等形态回退裸提及。
    pub fn strip_bot_mention(text: &str, bot_username: &str) -> String {
        if bot_username.is_empty() {
            return text.to_string();
        }
        let mention = format!("@{bot_username}");
        text.replace(&format!("{mention} "), "")
            .replace(&mention, "")
            .trim()
            .to_string()
    }

    /// 入站格式映射：Mattermost markdown → bot 内部 markdown。
    ///
    /// 两端同为标准 markdown 方言，正文恒等透传；只剥离 bot 自身提及
    /// （否则「@bot 帮我看下」的提及残留会污染模型输入）。
    pub fn to_internal_text(&self, raw: &str) -> String {
        let bot_username = self.bot_username.read().clone();
        Self::strip_bot_mention(raw, &bot_username)
    }

    /// 出站格式映射：bot 内部 markdown → Mattermost markdown（恒等透传）。
    ///
    /// 代码块、粗斜体、列表、引用、`:emoji:` 短代码双端语义一致；
    /// 如后续发现方言差异（如 Mattermost 特有 `!` 折叠语法），在此单点扩展。
    pub fn to_mattermost_markdown(content: &str) -> String {
        content.trim().to_string()
    }

    /// 校验 bot token（GET `users/me`），返回 (user_id, username)。
    async fn validate_bot_token(&self) -> Result<(String, String)> {
        let resp = self
            .http
            .get(self.api_url("/users/me"))
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("mattermost users/me failed: {e}")))?;

        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("mattermost users/me parse failed: {e}")))?;

        if !status.is_success() {
            let msg = body["message"].as_str().unwrap_or("unknown error");
            return Err(NemesisError::Channel(format!(
                "mattermost users/me failed: HTTP {status}: {msg}"
            )));
        }

        let user_id = body["id"].as_str().unwrap_or("").to_string();
        if user_id.is_empty() {
            return Err(NemesisError::Channel(
                "mattermost users/me response missing user id".to_string(),
            ));
        }
        let username = body["username"].as_str().unwrap_or("").to_string();
        Ok((user_id, username))
    }

    /// 出站：POST `/api/v4/posts`（channel_id + markdown 正文 + 可选 root_id）。
    async fn post_message(
        &self,
        channel_id: &str,
        message: &str,
        root_id: Option<&str>,
    ) -> Result<()> {
        let params = CreatePostParams {
            channel_id: channel_id.to_string(),
            message: message.to_string(),
            root_id: root_id.filter(|s| !s.is_empty()).map(String::from),
        };

        let resp = self
            .http
            .post(self.api_url("/posts"))
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .json(&params)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("mattermost post failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "mattermost posts API error: HTTP {status}: {text}"
            )));
        }

        Ok(())
    }

    /// 启动 WebSocket 接收循环（断线指数退避重连，slack 同款骨架）。
    fn start_ws_loop(&self) {
        let ctx = WsCtx {
            bot_token: self.config.bot_token.clone(),
            bot_user_id: self.bot_user_id.clone(),
            bot_username: self.bot_username.clone(),
            allow_from: self.config.allow_from.clone(),
            listen_channels: self.config.channels.clone(),
            bus_sender: self.bus_sender.clone(),
            running: self.running.clone(),
        };
        let base_url = self.config.base_url.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            let mut backoff = INITIAL_BACKOFF;

            loop {
                if !*running.read() {
                    break;
                }

                let ws_url = MattermostChannel::ws_url_from_base(&base_url);
                info!("[MattermostChannel] 连接 WebSocket 网关: {ws_url}");

                let ws_stream = match tokio_tungstenite::connect_async(&ws_url).await {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        warn!("[MattermostChannel] WebSocket 连接失败: {e}，{backoff:?} 后重试");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                };

                let session_started = std::time::Instant::now();
                let need_reconnect = MattermostChannel::ws_session(ws_stream, ctx.clone()).await;

                if !need_reconnect || !*running.read() {
                    break;
                }

                // 会话持续超过阈值 = 鉴权通过且健康运行后正常断开 → 重置退避
                // 快速重连；短命会话（连接成功但鉴权 FAIL 即返）保持指数退避——
                // 否则 token 失效时「连接成功即复位」会形成 1 次/秒的永久重试
                // 循环，MAX_BACKOFF 永远不生效（2026-09-26 复查修复）。
                if session_started.elapsed() > std::time::Duration::from_secs(60) {
                    backoff = INITIAL_BACKOFF;
                }

                warn!("[MattermostChannel] 连接断开，{backoff:?} 后重连");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }

            info!("[MattermostChannel] WebSocket 循环退出");
        });
    }

    /// WS 单连接会话：鉴权 challenge → 事件消费直到断开。
    ///
    /// 泛型 S 同时覆盖客户端（`MaybeTlsStream<TcpStream>`）与测试侧服务端
    /// （裸 `TcpStream`）两条路径。
    ///
    /// 返回 `true` 表示需要重连（对端关闭 / 错误 / 鉴权失败）。
    async fn ws_session<S>(ws: tokio_tungstenite::WebSocketStream<S>, ctx: WsCtx) -> bool
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        use tokio_tungstenite::tungstenite::Message;

        let (mut ws_tx, mut ws_rx) = ws.split();

        // —— 鉴权 challenge：连接后首帧必须认证（bot token 模式）
        let auth = MattermostChannel::build_auth_message(&ctx.bot_token);
        if let Err(e) = ws_tx.send(Message::Text(auth.into())).await {
            warn!("[MattermostChannel] 鉴权帧发送失败: {e}");
            return true;
        }

        // —— 等鉴权应答：status=OK 才进入事件流；FAIL 通常是 token 失效
        loop {
            let text = match ws_rx.next().await {
                Some(Ok(Message::Text(t))) => t.to_string(),
                Some(Ok(Message::Close(_))) => {
                    info!("[MattermostChannel] 鉴权期间被服务端关闭");
                    return true;
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => {
                    warn!("[MattermostChannel] WebSocket 鉴权阶段错误: {e}");
                    return true;
                }
                None => {
                    info!("[MattermostChannel] 鉴权期间连接关闭");
                    return true;
                }
            };

            let payload: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    debug!("[MattermostChannel] 鉴权阶段非 JSON 帧: {e}");
                    continue;
                }
            };

            match payload["status"].as_str() {
                Some("OK") => break,
                Some("FAIL") => {
                    let err = payload["error"]["message"].as_str().unwrap_or("unknown");
                    error!("[MattermostChannel] WebSocket 鉴权失败: {err}");
                    return true;
                }
                _ => continue,
            }
        }

        debug!("[MattermostChannel] WebSocket 鉴权通过，开始消费事件流");

        // —— 事件消费主循环
        loop {
            let text = tokio::select! {
                // running 粒度轮询（200ms tick）：空闲长连接（无消息时
                // ws_rx.next() 可挂很久）下 stop() 也能即时打断会话——否则
                // 旧会话继续收发直到连接自然断开，随后的 start() 再起第二条
                // 会话 → 双会话重复入站、旧会话重连循环永久化（nostr 同款）。
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                    if !*ctx.running.read() {
                        info!("[MattermostChannel] stop() 置位，事件循环退出");
                        return false;
                    }
                    continue;
                }
                msg = ws_rx.next() => match msg {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Close(_))) => {
                        info!("[MattermostChannel] WebSocket 被服务端关闭");
                        return true;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        warn!("[MattermostChannel] WebSocket 错误: {e}");
                        return true;
                    }
                    None => {
                        info!("[MattermostChannel] WebSocket 连接关闭");
                        return true;
                    }
                }
            };

            let payload: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[MattermostChannel] 非 JSON 帧丢弃: {e}");
                    continue;
                }
            };

            match payload["event"].as_str().unwrap_or("") {
                "hello" => {
                    debug!("[MattermostChannel] hello 事件");
                }

                "posted" => {
                    // users/me 校验失败时 bot 身份未知：WS 回声无法过滤，
                    // 放行会形成自问自答放大回路（allow_from 为空全放行时
                    // 尤甚）——fail-closed 丢弃，身份已知后自动恢复。
                    if ctx.bot_user_id.read().is_empty() {
                        warn!(
                            "[MattermostChannel] bot 身份未知（users/me 未通过），\
                             posted 事件丢弃以防自回声回路"
                        );
                        continue;
                    }
                    let inbound = MattermostChannel::parse_posted_event(
                        &payload,
                        &ctx.bot_user_id.read().clone(),
                        &ctx.bot_username.read().clone(),
                        &ctx.allow_from,
                        &ctx.listen_channels,
                    );
                    if let Some(inbound) = inbound {
                        debug!(
                            sender = %inbound.sender_id,
                            chat = %inbound.chat_id,
                            "[MattermostChannel] 入站消息"
                        );
                        if ctx.bus_sender.send(inbound).is_err() {
                            warn!("[MattermostChannel] 入站消息发布失败（bus 无接收者）");
                        }
                    }
                }

                other => {
                    debug!(event = other, "[MattermostChannel] 忽略事件");
                }
            }
        }
    }

    /// 解析 `posted` 事件为 `InboundMessage`（不满足过滤条件返回 None）。
    ///
    /// `data.post` 官方形态是**序列化后的 JSON 字符串**；兼容直接给对象的
    /// 非标准形态（部分版本/网关实现差异）。
    fn parse_posted_event(
        payload: &serde_json::Value,
        bot_user_id: &str,
        bot_username: &str,
        allow_from: &[String],
        listen_channels: &[String],
    ) -> Option<InboundMessage> {
        let event_type = payload["event"].as_str()?;
        if event_type != "posted" {
            return None;
        }

        let data = &payload["data"];
        let post: serde_json::Value = match data.get("post") {
            Some(serde_json::Value::String(s)) => serde_json::from_str(s).ok()?,
            Some(v @ serde_json::Value::Object(_)) => v.clone(),
            _ => return None,
        };

        // 系统消息（system_join_channel 等）不是用户对话，跳过
        let post_type = post["type"].as_str().unwrap_or("");
        if post_type.starts_with("system") {
            return None;
        }

        // 自己发出的消息会经 WS 回声，必须过滤（防自问自答死循环）
        let user_id = post["user_id"].as_str().unwrap_or("");
        if user_id.is_empty() {
            return None;
        }
        if !bot_user_id.is_empty() && user_id == bot_user_id {
            return None;
        }

        // 其他 bot / webhook 集成的消息跳过（props 里是字符串 "true"）
        let props = &post["props"];
        if prop_flag(props, "from_bot") || prop_flag(props, "from_webhook") {
            return None;
        }

        let channel_id = post["channel_id"]
            .as_str()
            .or_else(|| data["channel_id"].as_str())
            .unwrap_or("");
        if channel_id.is_empty() {
            return None;
        }
        let channel_name = data["channel_name"].as_str().unwrap_or("");

        // 频道映射过滤（channels 配置；空 = 全部）
        if !listen_channels.is_empty()
            && !listen_channels
                .iter()
                .any(|c| c == channel_id || (!channel_name.is_empty() && c == channel_name))
        {
            debug!(channel_id, "[MattermostChannel] 忽略未监听频道的消息");
            return None;
        }

        // 用户白名单过滤（allow_from；匹配 user_id 或用户名，空 = 全部）
        let sender_name = data["sender_name"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('@')
            .to_string();
        if !allow_from.is_empty()
            && !allow_from.iter().any(|u| {
                *u == user_id
                    || (!sender_name.is_empty()
                        && (*u == sender_name || *u == format!("@{sender_name}")))
            })
        {
            debug!("[MattermostChannel] 忽略白名单外用户 {user_id}");
            return None;
        }

        let text = post["message"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        let post_id = post["id"].as_str().unwrap_or("");
        let root_id = post["root_id"].as_str().unwrap_or("");

        let mut metadata = HashMap::new();
        metadata.insert("post_id".to_string(), post_id.to_string());
        metadata.insert("user_id".to_string(), user_id.to_string());
        if !channel_name.is_empty() {
            metadata.insert("channel_name".to_string(), channel_name.to_string());
        }
        if !sender_name.is_empty() {
            metadata.insert("sender_name".to_string(), sender_name.clone());
        }
        if !root_id.is_empty() {
            metadata.insert("root_id".to_string(), root_id.to_string());
        }

        // @提及检测（Mattermost 提及形态是 @username）
        if !bot_username.is_empty() && text.contains(&format!("@{bot_username}")) {
            metadata.insert("was_mentioned".to_string(), "true".to_string());
        }

        // 入站格式映射：剥离 bot 自身提及
        let content = MattermostChannel::strip_bot_mention(text, bot_username);

        // 复合 chat_id：线程回复带 root_id（对齐 slack 的 CHANNEL/THREAD_TS）
        let chat_id = if root_id.is_empty() {
            channel_id.to_string()
        } else {
            format!("{channel_id}/{root_id}")
        };

        Some(InboundMessage {
            channel: "mattermost".to_string(),
            sender_id: user_id.to_string(),
            chat_id,
            content,
            media: Vec::new(),
            session_key: String::new(),
            correlation_id: String::new(),
            metadata,
            voice_playback: None,
        })
    }
}

/// 读取 props 布尔旗标（Mattermost 官方序列化为字符串 `"true"`，兼容布尔）。
fn prop_flag(props: &serde_json::Value, key: &str) -> bool {
    match props.get(key) {
        Some(serde_json::Value::String(s)) => s == "true",
        Some(serde_json::Value::Bool(b)) => *b,
        _ => false,
    }
}

#[async_trait]
impl Channel for MattermostChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!("[MattermostChannel] starting (WebSocket gateway)");

        // 校验 bot token（users/me → bot 身份 + 提及检测用 username）
        match self.validate_bot_token().await {
            Ok((user_id, username)) => {
                *self.bot_user_id.write() = user_id.clone();
                *self.bot_username.write() = username.clone();
                info!(
                    "[MattermostChannel] bot 认证通过（user_id: {user_id}, username: @{username}）"
                );
            }
            Err(e) => {
                warn!("[MattermostChannel] users/me 校验失败（继续启动）: {e}");
            }
        }

        // 先置位再 spawn：tokio 多线程 runtime 下 spawn 的任务可能先于父任务
        // 之后的写执行——循环首行 `if !*running.read() { break; }` 会看到 false
        // 直接永久退出（2026-09-26 复查修复的启动竞态，slack 存量同款未动）。
        *self.running.write() = true;
        self.base.set_running(true);

        // 启动 WebSocket 接收循环
        self.start_ws_loop();

        self.base.set_enabled(true);
        info!("[MattermostChannel] started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[MattermostChannel] stopping");
        *self.running.write() = false;
        self.base.set_running(false);
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "mattermost channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let (channel_id, root_id) = Self::parse_chat_id(&msg.chat_id);
        if channel_id.is_empty() {
            return Err(NemesisError::Channel(format!(
                "invalid mattermost chat ID: {}",
                msg.chat_id
            )));
        }

        // 出站格式映射：bot 内部 markdown → Mattermost markdown
        let content = Self::to_mattermost_markdown(&msg.content);
        self.post_message(channel_id, &content, root_id).await?;
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

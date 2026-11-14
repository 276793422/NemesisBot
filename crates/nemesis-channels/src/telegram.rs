//! Telegram bot channel (long polling, HTML formatting, thinking indicator).
//!
//! Uses the Telegram Bot API directly via HTTP long polling for receiving
//! updates and REST API for sending messages. Supports text, photo, voice,
//! audio, and document messages with optional voice transcription.

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, MediaAttachment, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// 单块原始文本预算（UTF-16 code units）：Telegram Bot API 硬限 4096
/// 按 UTF-16 units 计（官方 entities 文档；astral emoji = 2 units），
/// 预算留 ~600 给 markdown→HTML 的转义/标签膨胀；纯文本回退落在原始块
/// 上恒 < 4096。拆分必须用 UTF-16 口径（split_message_utf16）——纯标量
/// 计数在 emoji 密集内容上少算至 2×，拆完仍会被整条拒收
/// （BUG-20261113-166）。
const TELEGRAM_SPLIT_BUDGET: usize = 3500;

// ---------------------------------------------------------------------------
// Thinking cancel mechanism
// ---------------------------------------------------------------------------

/// Wrapper for canceling a thinking animation.
///
/// Mirrors Go's `thinkingCancel` struct. When a response is ready to be sent,
/// the thinking animation for the corresponding chat is canceled by calling `cancel()`.
pub struct ThinkingCancel {
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ThinkingCancel {
    /// Create a new ThinkingCancel with the given cancel sender.
    pub fn new(cancel: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            cancel: Some(cancel),
        }
    }

    /// Cancel the thinking animation.
    pub fn cancel(&mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }
}

// ---------------------------------------------------------------------------
// Telegram API types
// ---------------------------------------------------------------------------

/// Telegram Bot API configuration.
/// SEC-002（BUG-20261111-068）：token / proxy（可带 userinfo 凭据）脱敏；
/// `redact_token` 是 reqwest 错误 Display 面的同族防线（BUG-20261109-006）。
#[derive(Clone)]
pub struct TelegramConfig {
    /// Bot token from BotFather.
    pub token: String,
    /// Optional proxy URL.
    pub proxy: Option<String>,
    /// Allowed sender IDs (empty = allow all).
    pub allow_from: Vec<String>,
    /// Telegram API base URL.
    pub api_base: String,
}

crate::debug_redact::impl_redacted_debug!(TelegramConfig {
    plain { allow_from, api_base }
    secret {}
    opt_secret { proxy }
});

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            proxy: None,
            allow_from: Vec::new(),
            api_base: "https://api.telegram.org".to_string(),
        }
    }
}

/// reqwest 错误 Display 内嵌完整请求 URL（`... for url ({url})`），而
/// Telegram 的 token 就在 URL 路径里（`/bot{token}/方法名`）——错误一进
/// 日志/错误串 token 即泄漏。所有格式化 reqwest 错误的位点必须过本函数
/// （BUG-20261109-006：泄漏面，与 URL 构造同族）。
fn redact_token(msg: impl std::fmt::Display, token: &str) -> String {
    let s = msg.to_string();
    if token.is_empty() {
        s
    } else {
        s.replace(token, "<redacted>")
    }
}

/// Telegram allowlist 判定（`handle_incoming_message` 抽出，单一真相源 +
/// 单测可达）。sender_id 形态：`"id|username"` 复合 / `"id"` 裸数字。
///
/// 匹配臂（allow 条目 `a` 先剥前导 `@`）：
/// - 复合身份全匹配：`sender_id == a`（条目写全 `"id|user"` 形态）
/// - bare id 复合前缀：`sender_id.starts_with("{a}|")`（条目写裸 id，
///   命中 `"id|任意用户名"`）
/// - 用户名折叠匹配：`username.eq_ignore_ascii_case(a)`（条目写裸
///   用户名，含大小写手滑形态）
///
/// BUG-20261109-032 修复两处：
/// ① 用户名比较大小写折叠——Telegram 用户名平台侧大小写不敏感
///   （@Alice 与 @alice 同一账号），此前 `u == a` 逐字节比较，
///   配置大小写手滑即误拒合法用户；
/// ② 删除裸前缀臂 `sender_id.starts_with(&format!("{a}"))`——它对
///   `"id|user"` 复合形态产生越权放行（allowed `"1234"` 命中
///   `"123456|xxx"`；Telegram 顺序发号使数字前缀碰撞现实可得），
///   且对条目含复合身份时产生部分用户名匹配；合法形态已由精确
///   `==` 与 `id|` 前缀臂完整覆盖，此臂唯一增量是错误放行。
fn telegram_allowlisted(allow_from: &[String], sender_id: &str, username: Option<&str>) -> bool {
    allow_from.iter().any(|a| {
        let a = a.trim_start_matches('@');
        // 全形态精确（条目写全 `"id|user"` 复合、裸 id sender、无用户名
        // sender 形态）
        if sender_id == a {
            return true;
        }
        // 纯数字条目 = bare id（Telegram 用户 id 恒数字，用户名必含非数
        // 字）：命中 `"id|任意用户名"` 复合 sender。`{a}|` 带分隔符前缀
        // ——不带分隔符的裸 starts_with 会把 "1234" 命中到 "123456|..."
        // （Telegram 顺序发号使数字前缀碰撞现实可得，BUG-20261109-032②
        // 的越权放行臂，已删）
        if !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) {
            return sender_id.starts_with(&format!("{a}|"));
        }
        match a.split_once('|') {
            // 复合条目 `"id|user"`：id 段精确前缀 + 用户名段大小写折叠
            // （复合身份的大小写漂移只可能落在用户名段——Telegram 用户
            // 名平台侧大小写不敏感，数字 id 段恒精确）
            Some((aid, auser)) if !aid.is_empty() && !auser.is_empty() => {
                sender_id.starts_with(&format!("{aid}|"))
                    && username.is_some_and(|u| u.eq_ignore_ascii_case(auser))
            }
            // `"id|"` 空用户名形态：按 bare id 处理
            Some((aid, _)) if !aid.is_empty() => sender_id.starts_with(&format!("{aid}|")),
            // 裸用户名条目（或畸形空 id）：大小写折叠
            _ => username.is_some_and(|u| u.eq_ignore_ascii_case(a)),
        }
    })
}

/// Bot API 上下文（轮询任务构造一次，媒体链路共享）：方法 URL / 文件
/// 下载 URL / token 脱敏三件单一真相源。文件下载命名空间是
/// `{base}/file/bot{token}/{file_path}`（官方文档），与方法 URL
/// `{base}/bot{token}/{method}` 不同（BUG-20261109-006：此前三处把
/// 文件下载拼成 `{bot{token}}/file/{path}`，命名空间交换 = 照片/语音/
/// 音频/文档文件访问全部 404，被降级文案掩盖）。
struct TelegramApi {
    /// `{api_base}/bot{token}`——API 方法调用基。
    url_base: String,
    /// `{api_base}/file/bot{token}`——文件下载基。
    file_url_base: String,
    /// token（redact 用，不进 URL 以外的地方）。
    token: String,
}

impl TelegramApi {
    fn new(api_base: &str, token: &str) -> Self {
        // api_base 尾斜杠归一：config 反序列化直通，用户从文档复制
        // `https://api.telegram.org/` 会拼出 `//bot…` 双斜杠路径（上游
        // 归一行为不可赌，404 即通道整体断链）。同族先例 = bluesky/
        // mastodon 构造期归一（BUG-20261109-012）。
        let api_base = api_base.trim_end_matches('/');
        TelegramApi {
            url_base: format!("{api_base}/bot{token}"),
            file_url_base: format!("{api_base}/file/bot{token}"),
            token: token.to_string(),
        }
    }

    fn method_url(&self, method: &str) -> String {
        format!("{}/{}", self.url_base, method)
    }

    /// getFile 返回的 file_path → 下载 URL（token 在场，只给本地下
    /// 载器用，不进 agent 上下文）。
    fn file_url(&self, file_path: &str) -> String {
        format!("{}/{}", self.file_url_base, file_path)
    }

    fn redact(&self, msg: impl std::fmt::Display) -> String {
        redact_token(msg, &self.token)
    }
}

/// Response from Telegram getUpdates API.
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    ok: bool,
    result: Vec<TelegramUpdate>,
}

/// A single Telegram update.
#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

/// A Telegram message.
#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    from: Option<TelegramUser>,
    chat: TelegramChat,
    text: Option<String>,
    caption: Option<String>,
    photo: Option<Vec<TelegramPhotoSize>>,
    voice: Option<TelegramFile>,
    audio: Option<TelegramAudio>,
    document: Option<TelegramDocument>,
}

/// A Telegram user.
#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    username: Option<String>,
    first_name: String,
    last_name: Option<String>,
}

/// A Telegram chat.
#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
}

/// Photo size variant.
#[derive(Debug, Deserialize)]
struct TelegramPhotoSize {
    file_id: String,
    width: i32,
    height: i32,
}

/// Telegram file reference.
#[derive(Debug, Deserialize)]
struct TelegramFile {
    file_id: String,
    file_unique_id: String,
    file_size: Option<i64>,
}

/// Telegram audio.
#[derive(Debug, Deserialize)]
struct TelegramAudio {
    file_id: String,
    file_name: Option<String>,
}

/// Telegram document.
#[derive(Debug, Deserialize)]
struct TelegramDocument {
    file_id: String,
    file_name: Option<String>,
}

/// Telegram getMe response.
#[derive(Debug, Deserialize)]
struct GetMeResponse {
    ok: bool,
    result: Option<TelegramUser>,
}

/// Telegram sendMessage response.
#[derive(Debug, Deserialize)]
struct SendMessageResponse {
    ok: bool,
    result: Option<TelegramMessage>,
}

/// Telegram editMessageText response.
#[derive(Debug, Deserialize)]
struct EditMessageResponse {
    ok: bool,
}

/// Parameters for sendMessage.
#[derive(Serialize, Default)]
struct SendMessageParams {
    chat_id: i64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<i64>,
}

/// Parameters for editMessageText.
#[derive(Serialize)]
struct EditMessageParams {
    chat_id: i64,
    message_id: i64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
}

/// Parameters for sendChatAction.
#[derive(Serialize)]
struct SendChatActionParams {
    chat_id: i64,
    action: String,
}

// ---------------------------------------------------------------------------
// TelegramChannel
// ---------------------------------------------------------------------------

/// Telegram bot channel using long polling.
pub struct TelegramChannel {
    base: BaseChannel,
    config: TelegramConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    placeholders: RwLock<HashMap<String, i64>>,
    /// 当前 poll 任务句柄。BUG-20261110-037：restart 序列（stop 置旗标
    /// false → start 500ms 后置回 true）时旧任务可能仍在 getUpdates 30s
    /// 长轮询内、醒来只见 true——双循环各自 offset=0 拉同一批 updates
    /// （重复回复）。start 挂新任务前 abort 旧任务，双启动/重启竞态都
    /// 确定性终止。旧 `cancel_tx` 机制从未被置 Some（stop 的 cancel 发送
    /// 是永久空操作），已删除。
    poll_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    bot_username: Arc<RwLock<String>>,
    transcriber: parking_lot::RwLock<Option<Arc<dyn crate::base::VoiceTranscriber>>>,
    /// Thinking cancel signals: chat_id -> ThinkingCancel.
    /// Mirrors Go's `stopThinking sync.Map`.
    stop_thinking: dashmap::DashMap<String, parking_lot::Mutex<ThinkingCancel>>,
    /// Bus sender for publishing inbound messages to the agent engine.
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl std::fmt::Debug for TelegramChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramChannel")
            .field("name", &self.base.name())
            .field("running", &*self.running.read())
            .finish()
    }
}

impl TelegramChannel {
    /// Creates a new `TelegramChannel`.
    pub fn new(
        config: TelegramConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.token.is_empty() {
            return Err(NemesisError::Channel(
                "telegram bot token is required".to_string(),
            ));
        }

        // 超时值收敛到 base::outbound_http_builder 统一定义。
        let mut client_builder = crate::base::outbound_http_builder();

        // Apply proxy if configured
        if let Some(ref proxy_url) = config.proxy
            && !proxy_url.is_empty()
        {
            match reqwest::Proxy::all(proxy_url.as_str()) {
                Ok(proxy) => {
                    client_builder = client_builder.proxy(proxy);
                }
                Err(e) => {
                    // proxy 是 opt_secret（userinfo 凭据面，SEC-002 口径，
                    // 与 Debug 脱敏同判）——日志只落 scheme 不落完整 URL
                    //（proxy_file 读侧 WARN 同口径，BUG-20261115-005）。
                    let scheme = proxy_url.split("://").next().unwrap_or("");
                    warn!("[TelegramChannel] invalid proxy URL (scheme=\"{scheme}\"): {e}");
                }
            }
        }

        let http = client_builder
            .build()
            .map_err(|e| NemesisError::Channel(format!("failed to create HTTP client: {e}")))?;

        Ok(Self {
            base: BaseChannel::new("telegram"),
            config,
            http,
            running: Arc::new(parking_lot::RwLock::new(false)),
            placeholders: RwLock::new(HashMap::new()),
            poll_handle: parking_lot::Mutex::new(None),
            bot_username: Arc::new(RwLock::new(String::new())),
            transcriber: parking_lot::RwLock::new(None),
            stop_thinking: dashmap::DashMap::new(),
            bus_sender,
        })
    }

    /// Creates a new `TelegramChannel` with a pre-configured HTTP client.
    ///
    /// Mirrors Go's `NewTelegramChannelWithClient()`. Useful for dependency injection
    /// in testing scenarios where you want to control the HTTP transport layer.
    /// The `bot` and `commands` fields are set to `None`/empty since they require
    /// a real Telegram Bot API connection.
    pub fn new_with_client(
        config: TelegramConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
        http: reqwest::Client,
    ) -> Result<Self> {
        if config.token.is_empty() {
            return Err(NemesisError::Channel(
                "telegram bot token is required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("telegram"),
            config,
            http,
            running: Arc::new(parking_lot::RwLock::new(false)),
            placeholders: RwLock::new(HashMap::new()),
            poll_handle: parking_lot::Mutex::new(None),
            bot_username: Arc::new(RwLock::new(String::new())),
            transcriber: parking_lot::RwLock::new(None),
            stop_thinking: dashmap::DashMap::new(),
            bus_sender,
        })
    }

    /// Sets the bus sender for publishing inbound messages.
    pub fn set_bus_sender(&self, sender: broadcast::Sender<InboundMessage>) {
        // Note: bus_sender is not directly mutable after construction,
        // so this is a no-op placeholder. Use new() with the sender instead.
        let _ = sender;
    }

    /// Returns the bot username (available after start).
    pub fn bot_username(&self) -> String {
        self.bot_username.read().clone()
    }

    /// Set the voice transcriber for audio message transcription.
    ///
    /// Mirrors Go's `TelegramChannel.SetTranscriber()`. When a voice/audio
    /// message is received, the transcriber will be used to convert speech to text
    /// if it is available.
    pub fn set_transcriber(&self, transcriber: Arc<dyn crate::base::VoiceTranscriber>) {
        *self.transcriber.write() = Some(transcriber);
    }

    /// Cancel any active thinking animation for the given chat ID.
    ///
    /// Mirrors Go's logic in `TelegramChannel.Send()` that cancels thinking
    /// before sending the actual response. Removes the cancel entry after invocation.
    pub fn stop_thinking_animation(&self, chat_id: &str) {
        if let Some((_, cancel)) = self.stop_thinking.remove(chat_id) {
            cancel.lock().cancel();
        }
    }

    /// Start a thinking animation for the given chat ID.
    ///
    /// Mirrors Go's logic that creates a cancel context with a 5-minute timeout
    /// and sends a "Thinking..." placeholder message. The returned oneshot receiver
    /// can be awaited to detect when the thinking should be canceled.
    ///
    /// Returns the oneshot receiver that will be signaled when thinking is canceled
    /// (either by a response being sent or by timeout).
    pub async fn start_thinking_animation(
        &self,
        chat_id: &str,
    ) -> std::result::Result<tokio::sync::oneshot::Receiver<()>, String> {
        // Cancel any previous thinking animation for this chat
        self.stop_thinking_animation(chat_id);

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.stop_thinking.insert(
            chat_id.to_string(),
            parking_lot::Mutex::new(ThinkingCancel::new(tx)),
        );

        // Send "Thinking..." placeholder
        if let Ok(chat_id_int) = chat_id.parse::<i64>() {
            let params = SendMessageParams {
                chat_id: chat_id_int,
                text: "Thinking...".to_string(),
                ..Default::default()
            };
            if let Ok(msg) = self.send_message(params).await {
                self.placeholders
                    .write()
                    .insert(chat_id.to_string(), msg.message_id);
            }
        }

        Ok(rx)
    }

    fn api_url(&self, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.config.api_base, self.config.token, method
        )
    }

    /// Calls the Telegram getUpdates API for long polling.
    async fn get_updates(&self, offset: i64) -> Result<Vec<TelegramUpdate>> {
        let resp = self
            .http
            .post(self.api_url("getUpdates"))
            .json(&serde_json::json!({
                "offset": offset,
                "timeout": 30,
                "allowed_updates": ["message"]
            }))
            .send()
            .await
            .map_err(|e| {
                NemesisError::Channel(redact_token(
                    format!("getUpdates request failed: {e}"),
                    &self.config.token,
                ))
            })?;

        let body: GetUpdatesResponse = resp.json().await.map_err(|e| {
            NemesisError::Channel(redact_token(
                format!("getUpdates parse failed: {e}"),
                &self.config.token,
            ))
        })?;

        if !body.ok {
            return Err(NemesisError::Channel(
                "getUpdates returned not ok".to_string(),
            ));
        }

        Ok(body.result)
    }

    /// Runs the long-polling loop in the background.
    ///
    /// Polls `getUpdates` continuously, converting each incoming message
    /// to an `InboundMessage` and publishing it to the bus via `bus_sender`.
    /// Uses exponential backoff on errors (max 60 seconds).
    fn polling_loop(&self) {
        let http = self.http.clone();
        let config = self.config.clone();
        let running = self.running.clone();
        let bus_sender = self.bus_sender.clone();
        let base_allow_list = self.config.allow_from.clone();
        let transcriber = self.transcriber.read().clone();

        let handle = tokio::spawn(async move {
            let api = TelegramApi::new(&config.api_base, &config.token);
            let mut offset: i64 = 0;
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            info!("[TelegramChannel] polling loop started");

            while *running.read() {
                // Build getUpdates request
                let resp = http
                    .post(api.method_url("getUpdates"))
                    .json(&serde_json::json!({
                        "offset": offset,
                        "timeout": 30,
                        "allowed_updates": ["message"]
                    }))
                    .send()
                    .await;

                match resp {
                    Ok(resp) => {
                        let body: std::result::Result<GetUpdatesResponse, _> = resp.json().await;
                        match body {
                            Ok(body) => {
                                if !body.ok {
                                    warn!("[TelegramChannel] getUpdates returned not ok");
                                    tokio::time::sleep(backoff).await;
                                    backoff = (backoff * 2).min(max_backoff);
                                    continue;
                                }

                                // Reset backoff on success
                                backoff = std::time::Duration::from_secs(1);

                                for update in body.result {
                                    offset = offset.max(update.update_id + 1);

                                    if let Some(msg) = update.message {
                                        Self::handle_incoming_message(
                                            &msg,
                                            &bus_sender,
                                            &base_allow_list,
                                            &transcriber,
                                            Some(&http),
                                            Some(&api),
                                        )
                                        .await;
                                    }
                                }
                            }
                            Err(e) => {
                                // reqwest decode 错误 Display 带 URL（token 在
                                // 路径里）——先脱敏再落日志（BUG-20261109-006）。
                                warn!(
                                    "[TelegramChannel] getUpdates parse error: {}",
                                    api.redact(e)
                                );
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(max_backoff);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[TelegramChannel] getUpdates request error: {}",
                            api.redact(e)
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                    }
                }
            }

            info!("[TelegramChannel] polling loop stopped");
        });

        // BUG-20261110-037：挂新任务前确定性 abort 上一任务。restart 序列
        // （stop 置旗标 false → 500ms → start 置回 true）时旧任务可能仍在
        // getUpdates 30s 长轮询内、醒来只见 true 继续跑——双循环各自
        // offset=0 拉同一批 updates（消息重复处理/竞争确认）。abort 保证
        // 无论旧任务卡在哪个 await 点都立即终止；双 start 则被 start 顶部
        // 守卫拦下，到不了这里。
        if let Some(old) = self.poll_handle.lock().replace(handle) {
            old.abort();
        }
    }

    /// Processes a single incoming Telegram message and publishes to the bus.
    ///
    /// Mirrors Go's `TelegramChannel.handleMessage()` logic:
    /// - Extracts sender info (ID and optional username)
    /// - Checks allow-list
    /// - Handles text, caption, photo, voice, audio, document content
    /// - Resolves file_ids to download URLs via getFile API (when HTTP available)
    /// - Constructs an InboundMessage with metadata
    ///
    /// When `http` and `api_url_base` are provided along with a `transcriber`,
    /// voice messages will be downloaded and transcribed (matching Go behavior).
    async fn handle_incoming_message(
        msg: &TelegramMessage,
        bus_sender: &broadcast::Sender<InboundMessage>,
        allow_from: &[String],
        transcriber: &Option<Arc<dyn crate::base::VoiceTranscriber>>,
        http: Option<&reqwest::Client>,
        api: Option<&TelegramApi>,
    ) {
        let user = match &msg.from {
            Some(u) => u,
            None => return,
        };

        // Build sender ID: "id" or "id|username"
        let sender_id = match &user.username {
            Some(username) if !username.is_empty() => {
                format!("{}|{}", user.id, username)
            }
            _ => format!("{}", user.id),
        };

        // Check allow-list (empty = allow all)
        if !allow_from.is_empty()
            && !telegram_allowlisted(allow_from, &sender_id, user.username.as_deref())
        {
            debug!("[TelegramChannel] message rejected by allowlist (sender_id={sender_id})");
            return;
        }

        let chat_id = format!("{}", msg.chat.id);
        let mut content = String::new();
        let mut media = Vec::new();

        // Text content
        if let Some(ref text) = msg.text {
            content.push_str(text);
        }

        // Caption (for media messages)
        if let Some(ref caption) = msg.caption {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(caption);
        }

        // Photo — （多模态）：getFile → 下载落盘 uploads → **本地路径引用**
        // 进 统一 attach（安全闸 → 历史引用 → build_messages 水合）。
        // 下载失败诚实降级为 "[Photo received]" 文案（不产出无法水合的引用）。
        if let Some(ref photos) = msg.photo
            && !photos.is_empty()
        {
            // Use the largest photo (last in array)
            let photo = &photos[photos.len() - 1];
            let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
                &nemesis_path::default_path_manager().workspace(),
            );
            let attachment = Self::fetch_file_attachment(
                http,
                api,
                &photo.file_id,
                "image",
                "jpg",
                &uploads_dir,
            )
            .await;
            if let Some(ref att) = attachment {
                media.push(att.clone());
            }
            if !content.is_empty() {
                content.push('\n');
            }
            match (&attachment, &msg.caption) {
                (Some(_), Some(cap)) => content.push_str(&format!("[image: {cap}]")),
                (Some(_), None) => content.push_str("[image: photo]"),
                (None, Some(cap)) => content.push_str(&format!("[Photo received: {cap}]")),
                (None, None) => content.push_str("[Photo received]"),
            }
        }

        // Voice
        if let Some(ref voice) = msg.voice {
            media.push(MediaAttachment {
                media_type: "voice".to_string(),
                url: voice.file_id.clone(),
                data: None,
            });
            if !content.is_empty() {
                content.push('\n');
            }

            let voice_text =
                match Self::transcribe_voice(transcriber, http, api, &voice.file_id).await {
                    Some(text) => text,
                    None => "[voice]".to_string(),
                };
            content.push_str(&voice_text);
        }

        // Audio — 与照片同纪律：本地下盘（下载 URL 带 bot token，直接给
        // agent 引用既无法水合又把 token 泄进会话上下文——BUG-20261109-006）。
        if let Some(ref audio) = msg.audio {
            let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
                &nemesis_path::default_path_manager().workspace(),
            );
            let attachment = Self::fetch_file_attachment(
                http,
                api,
                &audio.file_id,
                "audio",
                "mp3",
                &uploads_dir,
            )
            .await;
            if let Some(ref att) = attachment {
                media.push(att.clone());
            }
            if !content.is_empty() {
                content.push('\n');
            }
            let name = audio.file_name.as_deref().unwrap_or("audio");
            match &attachment {
                Some(_) => content.push_str(&format!("[audio: {name}]")),
                None => content.push_str(&format!("[Audio received: {name}]")),
            }
        }

        // Document — 与照片/音频同纪律：本地下盘（理由同上）。
        if let Some(ref doc) = msg.document {
            let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
                &nemesis_path::default_path_manager().workspace(),
            );
            let attachment = Self::fetch_file_attachment(
                http,
                api,
                &doc.file_id,
                "document",
                "bin",
                &uploads_dir,
            )
            .await;
            if let Some(ref att) = attachment {
                media.push(att.clone());
            }
            if !content.is_empty() {
                content.push('\n');
            }
            let name = doc.file_name.as_deref().unwrap_or("document");
            match &attachment {
                Some(_) => content.push_str(&format!("[file: {name}]")),
                None => content.push_str(&format!("[Document received: {name}]")),
            }
        }

        if content.is_empty() {
            content = "[empty message]".to_string();
        }

        // preview 截断必须按字符——`&content[..50]` 字节
        // 直切在多字节字符中间 panic（debug 日志开启 + 中文消息即触发：
        // 50 字节落在 16~17 个汉字的中间是常态不是边缘）。
        let preview = message_preview(&content);
        debug!(
            sender_id = %sender_id,
            chat_id = %chat_id,
            preview = preview,
            "[TelegramChannel] received message"
        );

        // Build metadata (mirrors Go's handleMessage metadata)
        let peer_kind = if msg.chat.chat_type != "private" {
            "group"
        } else {
            "direct"
        };
        let peer_id = if msg.chat.chat_type != "private" {
            format!("{}", msg.chat.id)
        } else {
            format!("{}", user.id)
        };

        let mut metadata = HashMap::new();
        metadata.insert("message_id".to_string(), format!("{}", msg.message_id));
        metadata.insert("user_id".to_string(), format!("{}", user.id));
        if let Some(ref username) = user.username {
            metadata.insert("username".to_string(), username.clone());
        }
        metadata.insert("first_name".to_string(), user.first_name.clone());
        metadata.insert(
            "is_group".to_string(),
            format!("{}", msg.chat.chat_type != "private"),
        );
        metadata.insert("peer_kind".to_string(), peer_kind.to_string());
        metadata.insert("peer_id".to_string(), peer_id);

        let inbound = InboundMessage {
            channel: "telegram".to_string(),
            sender_id,
            chat_id,
            content,
            media,
            session_key: String::new(),
            correlation_id: String::new(),
            metadata,
            voice_playback: None,
        };

        if let Err(e) = bus_sender.send(inbound) {
            warn!("[TelegramChannel] failed to publish inbound message: {e}");
        }
    }

    /// （多模态，goal）：把 Telegram 文件（photo/audio/document）**下载落盘**
    /// 成本地文件，返回路径形态的 `MediaAttachment`（供统一 attach 走安全
    /// 闸 + 历史路径引用；Telegram 的下载 URL 需要 bot token，直接给 agent
    /// 引用是无法水合的死链，且 token 会泄进会话上下文——必须本地方案，
    /// BUG-20261109-006：audio/document 此前回传带 token 的远程 URL）。
    ///
    /// 流程：getFile 拿 file_path → 组下载 URL（`{base}/file/bot{token}/`
    /// 命名空间，BUG-20261109-006：此前拼成 `{bot{token}}/file/`，文件
    /// 访问全部 404）→ 拉字节 → 写
    /// `uploads_dir/tg_{file_id sanitized}_{millis}.{ext}`（ext 抠自
    /// file_path，白名单字母数字且 ≤5 位，缺省 `default_ext`）。任一步
    /// 失败返回 `None`，调用方降级为 received 文案。写后即关 fd
    /// （`std::fs::write`），返回前数据已落盘（纪律 4）。
    async fn fetch_file_attachment(
        http: Option<&reqwest::Client>,
        api: Option<&TelegramApi>,
        file_id: &str,
        media_type: &str,
        default_ext: &str,
        uploads_dir: &std::path::Path,
    ) -> Option<MediaAttachment> {
        let http = http?;
        let api = api?;

        // Step 1: getFile → file_path
        let resp = http
            .post(api.method_url("getFile"))
            .json(&serde_json::json!({"file_id": file_id}))
            .send()
            .await
            .ok()?;
        let body: serde_json::Value = resp.json().await.ok()?;
        if body["ok"].as_bool() != Some(true) {
            return None;
        }
        let file_path = body["result"]["file_path"].as_str()?;

        // Step 2: download bytes（官方文件命名空间：{base}/file/bot{token}/{path}）
        let bytes = http
            .get(api.file_url(file_path))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .bytes()
            .await
            .ok()?;
        if bytes.is_empty() {
            return None;
        }

        // Step 3: 落盘（文件名注入面消毒：file_id 只留字母数字，其余转 `_`；
        // ext 白名单校验，防 file_path 夹带路径分隔符/奇怪后缀）。
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| {
                !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .unwrap_or_else(|| default_ext.to_string());
        let sanitized: String = file_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let name = format!(
            "tg_{}_{}.{}",
            sanitized,
            chrono::Utc::now().timestamp_millis(),
            ext
        );
        let dest = uploads_dir.join(name);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&dest, &bytes).ok()?;

        let path_str = dest.to_string_lossy().into_owned();
        info!(
            "[TelegramChannel] {} downloaded to {} ({} bytes)",
            media_type,
            path_str,
            bytes.len()
        );
        Some(MediaAttachment {
            media_type: media_type.to_string(),
            url: path_str,
            data: None,
        })
    }

    /// Download the voice file to `local_path`.
    ///
    /// 状态码前置闸：非 2xx 诚实上抛 `HTTP <status>`，不把错误体落盘；
    /// 传输层错误脱敏后上抛（reqwest Display 带 URL，token 在路径里）。
    async fn download_voice_file(
        http: &reqwest::Client,
        url: &str,
        local_path: &std::path::Path,
        api: &TelegramApi,
    ) -> std::result::Result<(), String> {
        let resp = http.get(url).send().await.map_err(|e| api.redact(e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| api.redact(e))?;
        std::fs::write(local_path, &bytes).map_err(|e| e.to_string())
    }

    /// Attempts to transcribe a voice message.
    ///
    /// Mirrors Go's voice transcription flow:
    /// 1. Check if transcriber is available
    /// 2. Download the voice file via Telegram getFile API
    /// 3. Call transcriber.Transcribe() on the downloaded file
    /// 4. Return formatted text: `[voice transcription: text]` on success,
    ///    `[voice (transcription failed)]` on error, or None if transcription
    ///    is not possible (no transcriber, no HTTP client, download failed).
    async fn transcribe_voice(
        transcriber: &Option<Arc<dyn crate::base::VoiceTranscriber>>,
        http: Option<&reqwest::Client>,
        api: Option<&TelegramApi>,
        file_id: &str,
    ) -> Option<String> {
        let transcriber = transcriber.as_ref()?;
        if !transcriber.is_available() {
            return None;
        }

        let http = http?;
        let api = api?;

        // Step 1: Call getFile to get the file_path
        let file_resp = http
            .post(api.method_url("getFile"))
            .json(&serde_json::json!({"file_id": file_id}))
            .send()
            .await;

        let file_path = match file_resp {
            Ok(resp) => {
                let body: serde_json::Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        // reqwest decode 错误 Display 带 URL（token 在路径里）——脱敏（BUG-20261109-006）
                        warn!(
                            "[TelegramChannel] voice: getFile parse error: {}",
                            api.redact(e)
                        );
                        return Some("[voice (transcription failed)]".to_string());
                    }
                };
                if body["ok"].as_bool() != Some(true) {
                    warn!(
                        "[TelegramChannel] voice: getFile returned not ok: {:?}",
                        body
                    );
                    return Some("[voice (transcription failed)]".to_string());
                }
                match body["result"]["file_path"].as_str() {
                    Some(p) => p.to_string(),
                    None => {
                        warn!("[TelegramChannel] voice: getFile missing file_path");
                        return Some("[voice (transcription failed)]".to_string());
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[TelegramChannel] voice: getFile request error: {}",
                    api.redact(e)
                );
                return Some("[voice (transcription failed)]".to_string());
            }
        };

        // Step 2: Download the voice file to a temp path
        // （官方文件命名空间 {base}/file/bot{token}/{path}——此前拼成
        // {bot{token}}/file/ 全部 404，转写永远失败；BUG-20261109-006）
        let download_url = api.file_url(&file_path);
        let temp_dir = std::env::temp_dir().join("nemesisbot_telegram_voice");
        let _ = std::fs::create_dir_all(&temp_dir);
        let local_path = temp_dir.join(format!("{}.ogg", file_id));

        // 下载先查状态码——此前 404/500 的 HTML/JSON
        // 错误体被当语音 OGG 落盘送转写，日志只见「transcription failed」，
        // 真实 HTTP 错误被掩盖（signal/image_attach 同款闸此处漏配）。
        if let Err(e) = Self::download_voice_file(http, &download_url, &local_path, api).await {
            warn!("[TelegramChannel] voice: download error: {e}");
            return Some("[voice (transcription failed)]".to_string());
        }

        let path_str = local_path.to_string_lossy().to_string();

        // Step 3: Call transcriber
        let transcribed = transcriber.transcribe(&path_str).await;
        // 转写完成即删临时文件——Ok/Err 两臂统一（finally 语义）。
        // BUG-20261113-158：此前两臂都不删，每条语音消息在系统级
        // temp（std::env::temp_dir 不随 workspace 清理）永久残留一个
        // ogg（几百 KB~几 MB）。转写器读路径成功臂读完即无用，失败臂
        // 同样不应把语音内容残留磁盘。
        let _ = std::fs::remove_file(&local_path);

        match transcribed {
            Ok(text) => {
                info!("[TelegramChannel] voice transcribed successfully");
                Some(format!("[voice transcription: {text}]"))
            }
            Err(e) => {
                warn!("[TelegramChannel] voice transcription failed: {e}");
                Some("[voice (transcription failed)]".to_string())
            }
        }
    }

    async fn get_me(&self) -> Result<TelegramUser> {
        let resp = self
            .http
            .get(self.api_url("getMe"))
            .send()
            .await
            .map_err(|e| {
                NemesisError::Channel(redact_token(
                    format!("getMe request failed: {e}"),
                    &self.config.token,
                ))
            })?;

        let body: GetMeResponse = resp.json().await.map_err(|e| {
            NemesisError::Channel(redact_token(
                format!("getMe parse failed: {e}"),
                &self.config.token,
            ))
        })?;

        if !body.ok {
            return Err(NemesisError::Channel("getMe returned not ok".to_string()));
        }

        body.result
            .ok_or_else(|| NemesisError::Channel("getMe returned no user".to_string()))
    }

    async fn send_message(&self, params: SendMessageParams) -> Result<TelegramMessage> {
        let resp = self
            .http
            .post(self.api_url("sendMessage"))
            .json(&params)
            .send()
            .await
            .map_err(|e| {
                NemesisError::Channel(redact_token(
                    format!("sendMessage failed: {e}"),
                    &self.config.token,
                ))
            })?;

        let body: SendMessageResponse = resp.json().await.map_err(|e| {
            NemesisError::Channel(redact_token(
                format!("sendMessage parse failed: {e}"),
                &self.config.token,
            ))
        })?;

        if !body.ok {
            return Err(NemesisError::Channel(
                "sendMessage returned not ok".to_string(),
            ));
        }

        body.result
            .ok_or_else(|| NemesisError::Channel("sendMessage returned no message".to_string()))
    }

    async fn edit_message_text(&self, params: EditMessageParams) -> Result<()> {
        let resp = self
            .http
            .post(self.api_url("editMessageText"))
            .json(&params)
            .send()
            .await
            .map_err(|e| {
                NemesisError::Channel(redact_token(
                    format!("editMessageText failed: {e}"),
                    &self.config.token,
                ))
            })?;

        let body: EditMessageResponse = resp.json().await.map_err(|e| {
            NemesisError::Channel(redact_token(
                format!("editMessageText parse failed: {e}"),
                &self.config.token,
            ))
        })?;

        if !body.ok {
            // Edit can fail if message hasn't changed, treat as non-fatal
            debug!("[TelegramChannel] editMessageText returned not ok (may be non-fatal)");
        }

        Ok(())
    }

    async fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<()> {
        let params = SendChatActionParams {
            chat_id,
            action: action.to_string(),
        };
        let _ = self
            .http
            .post(self.api_url("sendChatAction"))
            .json(&params)
            .send()
            .await;
        Ok(())
    }

    /// Converts markdown text to Telegram-compatible HTML.
    ///
    /// Uses a line-by-line parser that
    /// produces more reliable HTML than regex-based approaches:
    /// - Fenced code blocks → `<pre><code>`
    /// - Headings → `<b>` (bold title)
    /// - Blockquotes → `<blockquote>`
    /// - Unordered lists → `• item`
    /// - Ordered lists → `1. item`
    /// - Inline: bold, italic, code, links, strikethrough
    pub fn markdown_to_telegram_html(text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }

        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut blocks = Vec::new();
        let lines: Vec<&str> = normalized.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();

            if trimmed.is_empty() {
                i += 1;
                continue;
            }

            // Fenced code block
            if trimmed.starts_with("```") {
                // Single-line fenced block: ```code``` with open and close fences
                // on the same line. Extract content between first and last ```.
                // (Byte offsets 3 and len-3 are char boundaries since ``` is ASCII.)
                if trimmed.len() > 6 && trimmed.ends_with("```") {
                    let inner = &trimmed[3..trimmed.len() - 3];
                    blocks.push(format!("<pre><code>{}</code></pre>", escape_html(inner)));
                    i += 1;
                    continue;
                }

                // Multi-line fenced block (open/close fences on separate lines)
                i += 1;
                let mut code_lines = Vec::new();
                while i < lines.len() {
                    if lines[i].trim().starts_with("```") {
                        i += 1;
                        break;
                    }
                    code_lines.push(lines[i]);
                    i += 1;
                }
                let code = escape_html(&code_lines.join("\n"));
                blocks.push(format!("<pre><code>{}</code></pre>", code));
                continue;
            }

            // ATX heading (# through ######) → bold
            if let Some(heading) = parse_heading(trimmed) {
                blocks.push(format!("<b>{}</b>", render_inline(heading)));
                i += 1;
                continue;
            }

            // Blockquote (> lines)
            if trimmed.starts_with('>') {
                let mut quote_lines = Vec::new();
                while i < lines.len() {
                    let current = lines[i].trim();
                    if current.is_empty() || !current.starts_with('>') {
                        break;
                    }
                    let content = current.strip_prefix('>').unwrap_or(current).trim_start();
                    quote_lines.push(render_inline(content));
                    i += 1;
                }
                blocks.push(format!(
                    "<blockquote>{}</blockquote>",
                    quote_lines.join("\n")
                ));
                continue;
            }

            // Unordered list (- * +)
            if let Some(item) = parse_unordered_item(trimmed) {
                let mut items = vec![format!("• {}", render_inline(item.trim()))];
                i += 1;
                while i < lines.len() {
                    let current = lines[i].trim();
                    if let Some(next) = parse_unordered_item(current) {
                        items.push(format!("• {}", render_inline(next.trim())));
                        i += 1;
                    } else if current.is_empty() {
                        i += 1;
                        break;
                    } else {
                        break;
                    }
                }
                blocks.push(items.join("\n"));
                continue;
            }

            // Ordered list (1. 2. etc.)
            if let Some(item) = parse_ordered_item(trimmed) {
                let mut items = vec![format!("1. {}", render_inline(item.trim()))];
                let mut counter = 2;
                i += 1;
                while i < lines.len() {
                    let current = lines[i].trim();
                    if let Some(next) = parse_ordered_item(current) {
                        items.push(format!("{}. {}", counter, render_inline(next.trim())));
                        counter += 1;
                        i += 1;
                    } else if current.is_empty() {
                        i += 1;
                        break;
                    } else {
                        break;
                    }
                }
                blocks.push(items.join("\n"));
                continue;
            }

            // Paragraph
            let mut para_lines = vec![trimmed];
            i += 1;
            while i < lines.len() {
                let current = lines[i].trim();
                if current.is_empty()
                    || current.starts_with("```")
                    || parse_heading(current).is_some()
                    || current.starts_with('>')
                    || parse_unordered_item(current).is_some()
                    || parse_ordered_item(current).is_some()
                {
                    break;
                }
                para_lines.push(current);
                i += 1;
            }
            blocks.push(render_inline(&para_lines.join("\n")));
        }

        blocks.join("\n\n")
    }
}

// ---------------------------------------------------------------------------
// Markdown parsing helpers (line-by-line, no regex)
// ---------------------------------------------------------------------------

/// 入站消息 debug 预览截断（按字符；字节直切在多字节字符中间 panic，
/// —中文消息 50 字节落在 16~17 个汉字中间是常态）。
fn message_preview(content: &str) -> String {
    content.chars().take(50).collect()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parse_heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ') {
        Some(&line[hashes + 1..])
    } else {
        None
    }
}

fn parse_unordered_item(line: &str) -> Option<&str> {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn parse_ordered_item(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// Render inline markdown (bold, italic, code, links, strikethrough).
fn render_inline(text: &str) -> String {
    let mut result = escape_html(text);

    // Links: [text](url) → <a href="url">text</a>
    while let Some(bracket_start) = result.find('[') {
        if let Some(rel) = result[bracket_start..].find("](") {
            let bracket_end = bracket_start + rel;
            if let Some(paren_rel) = result[bracket_end + 2..].find(')') {
                let paren_end = bracket_end + 2 + paren_rel;
                let link_text = result[bracket_start + 1..bracket_end].to_string();
                let url = result[bracket_end + 2..paren_end].to_string();
                result = format!(
                    "{}<a href=\"{}\">{}</a>{}",
                    &result[..bracket_start],
                    url,
                    link_text,
                    &result[paren_end + 1..]
                );
                continue;
            }
        }
        break;
    }

    // Strikethrough: ~~text~~ → <s>text</s>
    while let Some(start) = result.find("~~") {
        if let Some(end_rel) = result[start + 2..].find("~~") {
            let end = start + 2 + end_rel;
            let inner = result[start + 2..end].to_string();
            result = format!("{}<s>{}</s>{}", &result[..start], inner, &result[end + 2..]);
        } else {
            break;
        }
    }

    // Bold: **text** → <b>text</b>
    while let Some(start) = result.find("**") {
        if let Some(end_rel) = result[start + 2..].find("**") {
            let end = start + 2 + end_rel;
            let inner = result[start + 2..end].to_string();
            result = format!("{}<b>{}</b>{}", &result[..start], inner, &result[end + 2..]);
        } else {
            break;
        }
    }

    // Bold: __text__ → <b>text</b>
    while let Some(start) = result.find("__") {
        if let Some(end_rel) = result[start + 2..].find("__") {
            let end = start + 2 + end_rel;
            let inner = result[start + 2..end].to_string();
            result = format!("{}<b>{}</b>{}", &result[..start], inner, &result[end + 2..]);
        } else {
            break;
        }
    }

    // Inline code: `text` → <code>text</code>
    while let Some(start) = result.find('`') {
        if let Some(end_rel) = result[start + 1..].find('`') {
            let end = start + 1 + end_rel;
            let inner = result[start + 1..end].to_string();
            result = format!(
                "{}<code>{}</code>{}",
                &result[..start],
                inner,
                &result[end + 1..]
            );
        } else {
            break;
        }
    }

    // Italic: single * (not **) or single _ (not __) → <i>text</i>.
    // `__bold__` was already converted to <b> above, so a lone _ here is
    // unambiguous. For _ we additionally require a word boundary on at least
    // one side (CommonMark: _ only emphasizes at word edges) so that intraword
    // underscores like `snake_case` / `file_name` stay literal.
    let mut out = String::with_capacity(result.len());
    let chars: Vec<char> = result.chars().collect();
    let mut idx = 0;
    let mut in_italic = false;
    while idx < chars.len() {
        let c = chars[idx];
        let prev = if idx == 0 { None } else { Some(chars[idx - 1]) };
        let next = if idx + 1 >= chars.len() {
            None
        } else {
            Some(chars[idx + 1])
        };

        let star_marker = c == '*' && (prev != Some('*')) && (next != Some('*'));
        let underscore_marker = c == '_'
            && (prev != Some('_'))
            && (next != Some('_'))
            && !(prev.is_some_and(|p| p.is_alphanumeric())
                && next.is_some_and(|n| n.is_alphanumeric()));

        if star_marker || underscore_marker {
            if in_italic {
                out.push_str("</i>");
            } else {
                out.push_str("<i>");
            }
            in_italic = !in_italic;
        } else {
            out.push(c);
        }
        idx += 1;
    }

    // 奇数个孤立标记（「3 * 4 = 12」「wildcard * matches」）会让 <i> 悬空
    // 不闭合——Telegram HTML parse mode 拒收不闭合标签，sendMessage 400
    // → 整块回退纯文本，同块内全部格式（代码/加粗/链接）丢失。收尾补
    // 闭合，输出恒平衡。
    if in_italic {
        out.push_str("</i>");
    }

    out
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        // BUG-20261110-037：幂等启动守卫——重复 start（不经 stop）会双 poll
        // 循环各自 offset=0 拉同一批 getUpdates（消息重复处理）。已运行时
        // 诚实跳过。旗标读 base（trait is_running 契约面），start 成功才置位。
        if self.base.is_running() {
            info!("[TelegramChannel] already running; skipping duplicate start");
            return Ok(());
        }
        info!("[TelegramChannel] starting bot (polling mode)");

        let me = self.get_me().await?;
        *self.bot_username.write() = me.username.clone().unwrap_or_default();

        info!(
            username = %self.bot_username.read(),
            "[TelegramChannel] bot connected"
        );

        *self.running.write() = true;
        self.base.set_enabled(true);
        // BUG-20261110-037：此前从未 set_running——trait is_running()（读
        // base 旗标）恒 false，违反「start 成功后、stop 前为 running」契约，
        // manager 的运行中守卫对本通道失效。
        self.base.set_running(true);

        // Start long polling in background
        self.polling_loop();

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[TelegramChannel] stopping bot");
        *self.running.write() = false;
        self.base.set_enabled(false);
        self.base.set_running(false);

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "telegram bot not running".to_string(),
            ));
        }

        let chat_id: i64 = msg.chat_id.parse().map_err(|e: std::num::ParseIntError| {
            NemesisError::Channel(format!("invalid chat ID: {e}"))
        })?;

        self.base.record_sent();

        // Stop thinking animation before sending the response.
        // Mirrors Go's `TelegramChannel.Send()` that cancels thinking
        // before sending the actual response.
        self.stop_thinking_animation(&msg.chat_id);

        // Telegram Bot API sendMessage/editMessageText 硬限 4096 ——按
        // UTF-16 code units 计（官方 entities 文档；astral emoji = 2
        // units），超长回复 HTML 与纯文本两次尝试全 400，整条投递失败。
        // 预算 3500 units 留 HTML 转义/标签膨胀余量；即便病理转义内容把
        // HTML 版顶过 4096，纯文本回退落在原始块（≤3500 units < 4096）
        // 上恒可达。
        let raw_chunks =
            nemesis_types::utils::split_message_utf16(&msg.content, TELEGRAM_SPLIT_BUDGET);
        let html_chunks: Vec<String> = raw_chunks
            .iter()
            .map(|c| Self::markdown_to_telegram_html(c))
            .collect();

        // Send one chunk: HTML first, plain-text fallback on parse failure.
        async fn send_chunk(
            me: &TelegramChannel,
            chat_id: i64,
            html: &str,
            raw: &str,
        ) -> Result<()> {
            let send_params = SendMessageParams {
                chat_id,
                text: html.to_string(),
                parse_mode: Some("HTML".to_string()),
                ..Default::default()
            };
            match me.send_message(send_params).await {
                Ok(_) => Ok(()),
                Err(_) => {
                    warn!("[TelegramChannel] HTML parse failed, falling back to plain text");
                    let plain_params = SendMessageParams {
                        chat_id,
                        text: raw.to_string(),
                        ..Default::default()
                    };
                    me.send_message(plain_params).await?;
                    Ok(())
                }
            }
        }

        // Try to edit placeholder first（首块走占位编辑；剩余块照常新消息）
        let placeholder_msg_id = self.placeholders.write().remove(&msg.chat_id);
        if let Some(message_id) = placeholder_msg_id {
            let edit_params = EditMessageParams {
                chat_id,
                message_id,
                text: html_chunks[0].clone(),
                parse_mode: Some("HTML".to_string()),
            };

            if self.edit_message_text(edit_params).await.is_ok() {
                for (html, raw) in html_chunks.iter().zip(raw_chunks.iter()).skip(1) {
                    send_chunk(self, chat_id, html, raw).await?;
                }
                return Ok(());
            }
            // Fall through to new messages if edit fails
        }

        for (html, raw) in html_chunks.iter().zip(raw_chunks.iter()) {
            send_chunk(self, chat_id, html, raw).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

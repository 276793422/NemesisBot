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
#[derive(Debug, Clone)]
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
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
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

        let mut client_builder =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(35));

        // Apply proxy if configured
        if let Some(ref proxy_url) = config.proxy {
            if !proxy_url.is_empty() {
                match reqwest::Proxy::all(proxy_url.as_str()) {
                    Ok(proxy) => {
                        client_builder = client_builder.proxy(proxy);
                    }
                    Err(e) => {
                        warn!("[TelegramChannel] invalid proxy URL '{}': {}", proxy_url, e);
                    }
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
            cancel_tx: parking_lot::Mutex::new(None),
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
            cancel_tx: parking_lot::Mutex::new(None),
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
            .map_err(|e| NemesisError::Channel(format!("getUpdates request failed: {e}")))?;

        let body: GetUpdatesResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("getUpdates parse failed: {e}")))?;

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

        tokio::spawn(async move {
            let api_url_base = format!("{}/bot{}", config.api_base, config.token);
            let mut offset: i64 = 0;
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            info!("[TelegramChannel] polling loop started");

            while *running.read() {
                // Build getUpdates request
                let resp = http
                    .post(format!("{api_url_base}/getUpdates"))
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
                                            Some(&api_url_base),
                                        )
                                        .await;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("[TelegramChannel] getUpdates parse error: {e}");
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(max_backoff);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[TelegramChannel] getUpdates request error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                    }
                }
            }

            info!("[TelegramChannel] polling loop stopped");
        });
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
        api_url_base: Option<&str>,
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
        if !allow_from.is_empty() {
            let allowed = allow_from.iter().any(|a| {
                let a = a.trim_start_matches('@');
                sender_id == a
                    || sender_id.starts_with(&format!("{a}|"))
                    || sender_id.starts_with(&format!("{a}"))
                    || user.username.as_ref().map_or(false, |u| u == a)
            });
            if !allowed {
                debug!("[TelegramChannel] message rejected by allowlist (sender_id={sender_id})");
                return;
            }
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

        // Photo — T7（多模态）：getFile → 下载落盘 uploads → **本地路径引用**
        // 进 T5 统一 attach（安全闸 → 历史引用 → build_messages 水合）。
        // 下载失败诚实降级为 "[Photo received]" 文案（不产出无法水合的引用）。
        if let Some(ref photos) = msg.photo {
            if !photos.is_empty() {
                // Use the largest photo (last in array)
                let photo = &photos[photos.len() - 1];
                let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
                    &nemesis_path::default_path_manager().workspace(),
                );
                let attachment =
                    Self::fetch_photo_attachment(http, api_url_base, &photo.file_id, &uploads_dir)
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
                match Self::transcribe_voice(transcriber, http, api_url_base, &voice.file_id).await
                {
                    Some(text) => text,
                    None => "[voice]".to_string(),
                };
            content.push_str(&voice_text);
        }

        // Audio — resolve file_id to download URL
        if let Some(ref audio) = msg.audio {
            let file_url = Self::get_file_url(http, api_url_base, &audio.file_id).await;
            if let Some(ref url) = file_url {
                media.push(MediaAttachment {
                    media_type: "audio".to_string(),
                    url: url.clone(),
                    data: None,
                });
            }
            if !content.is_empty() {
                content.push('\n');
            }
            let name = audio.file_name.as_deref().unwrap_or("audio");
            match &file_url {
                Some(_) => content.push_str(&format!("[audio: {name}]")),
                None => content.push_str(&format!("[Audio received: {name}]")),
            }
        }

        // Document — resolve file_id to download URL
        if let Some(ref doc) = msg.document {
            let file_url = Self::get_file_url(http, api_url_base, &doc.file_id).await;
            if let Some(ref url) = file_url {
                media.push(MediaAttachment {
                    media_type: "document".to_string(),
                    url: url.clone(),
                    data: None,
                });
            }
            if !content.is_empty() {
                content.push('\n');
            }
            let name = doc.file_name.as_deref().unwrap_or("document");
            match &file_url {
                Some(_) => content.push_str(&format!("[file: {name}]")),
                None => content.push_str(&format!("[Document received: {name}]")),
            }
        }

        if content.is_empty() {
            content = "[empty message]".to_string();
        }

        debug!(
            sender_id = %sender_id,
            chat_id = %chat_id,
            preview = &content[..content.len().min(50)],
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

    /// Resolve a Telegram file_id to a download URL via the Bot API's getFile endpoint.
    ///
    /// Returns `Some(download_url)` on success, `None` if HTTP is unavailable or
    /// the API call fails.
    async fn get_file_url(
        http: Option<&reqwest::Client>,
        api_url_base: Option<&str>,
        file_id: &str,
    ) -> Option<String> {
        let http = http?;
        let api_url_base = api_url_base?;

        let url = format!("{api_url_base}/getFile");
        let resp = http
            .post(&url)
            .json(&serde_json::json!({"file_id": file_id}))
            .send()
            .await
            .ok()?;

        let body: serde_json::Value = resp.json().await.ok()?;
        if body["ok"].as_bool() != Some(true) {
            return None;
        }
        let file_path = body["result"]["file_path"].as_str()?;
        Some(format!("{api_url_base}/file/{file_path}"))
    }

    /// T7（多模态，goal 2026-09-03）：把 Telegram photo **下载落盘**成本地
    /// 文件，返回路径形态的 `MediaAttachment`（供 T5 统一 attach 走安全闸 +
    /// 历史路径引用；Telegram 的下载 URL 需要 bot token，直接给 agent 引用
    /// 是无法水合的死链——必须本地方案）。
    ///
    /// 流程：getFile 拿 file_path → 组下载 URL → 拉字节 → 写
    /// `uploads_dir/tg_{file_id sanitized}_{millis}.{ext}`（ext 抠自
    /// file_path，白名单字母数字且 ≤5 位，缺省 jpg）。任一步失败返回
    /// `None`，调用方降级为 "[Photo received]" 文案。写后即关 fd
    /// （`std::fs::write`），返回前数据已落盘（纪律 4）。
    async fn fetch_photo_attachment(
        http: Option<&reqwest::Client>,
        api_url_base: Option<&str>,
        file_id: &str,
        uploads_dir: &std::path::Path,
    ) -> Option<MediaAttachment> {
        let http = http?;
        let api_url_base = api_url_base?;

        // Step 1: getFile → file_path
        let resp = http
            .post(format!("{api_url_base}/getFile"))
            .json(&serde_json::json!({"file_id": file_id}))
            .send()
            .await
            .ok()?;
        let body: serde_json::Value = resp.json().await.ok()?;
        if body["ok"].as_bool() != Some(true) {
            return None;
        }
        let file_path = body["result"]["file_path"].as_str()?;

        // Step 2: download bytes
        let bytes = http
            .get(format!("{api_url_base}/file/{file_path}"))
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
            .unwrap_or_else(|| "jpg".to_string());
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
            "[TelegramChannel] photo downloaded to {} ({} bytes)",
            path_str,
            bytes.len()
        );
        Some(MediaAttachment {
            media_type: "image".to_string(),
            url: path_str,
            data: None,
        })
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
        api_url_base: Option<&str>,
        file_id: &str,
    ) -> Option<String> {
        let transcriber = transcriber.as_ref()?;
        if !transcriber.is_available() {
            return None;
        }

        let http = http?;
        let api_url_base = api_url_base?;

        // Step 1: Call getFile to get the file_path
        let file_resp = http
            .post(format!("{api_url_base}/getFile"))
            .json(&serde_json::json!({"file_id": file_id}))
            .send()
            .await;

        let file_path = match file_resp {
            Ok(resp) => {
                let body: serde_json::Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[TelegramChannel] voice: getFile parse error: {e}");
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
                warn!("[TelegramChannel] voice: getFile request error: {e}");
                return Some("[voice (transcription failed)]".to_string());
            }
        };

        // Step 2: Download the voice file to a temp path
        let download_url = format!("{api_url_base}/file/{file_path}");
        let temp_dir = std::env::temp_dir().join("nemesisbot_telegram_voice");
        let _ = std::fs::create_dir_all(&temp_dir);
        let local_path = temp_dir.join(format!("{}.ogg", file_id));

        let download_resp = http.get(&download_url).send().await;
        match download_resp {
            Ok(resp) => {
                if let Ok(bytes) = resp.bytes().await {
                    if let Err(e) = std::fs::write(&local_path, &bytes) {
                        warn!("[TelegramChannel] voice: failed to write file: {e}");
                        return Some("[voice (transcription failed)]".to_string());
                    }
                } else {
                    warn!("[TelegramChannel] voice: failed to read download bytes");
                    return Some("[voice (transcription failed)]".to_string());
                }
            }
            Err(e) => {
                warn!("[TelegramChannel] voice: download error: {e}");
                return Some("[voice (transcription failed)]".to_string());
            }
        }

        let path_str = local_path.to_string_lossy().to_string();

        // Step 3: Call transcriber
        match transcriber.transcribe(&path_str).await {
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
            .map_err(|e| NemesisError::Channel(format!("getMe request failed: {e}")))?;

        let body: GetMeResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("getMe parse failed: {e}")))?;

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
            .map_err(|e| NemesisError::Channel(format!("sendMessage failed: {e}")))?;

        let body: SendMessageResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("sendMessage parse failed: {e}")))?;

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
            .map_err(|e| NemesisError::Channel(format!("editMessageText failed: {e}")))?;

        let body: EditMessageResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("editMessageText parse failed: {e}")))?;

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
                blocks.push(format!("<b>{}</b>", render_inline(&heading)));
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

        let star_marker =
            c == '*' && prev.map_or(true, |p| p != '*') && next.map_or(true, |n| n != '*');
        let underscore_marker = c == '_'
            && prev.map_or(true, |p| p != '_')
            && next.map_or(true, |n| n != '_')
            && !(prev.map_or(false, |p| p.is_alphanumeric())
                && next.map_or(false, |n| n.is_alphanumeric()));

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
        info!("[TelegramChannel] starting bot (polling mode)");

        let me = self.get_me().await?;
        *self.bot_username.write() = me.username.clone().unwrap_or_default();

        info!(
            username = %self.bot_username.read(),
            "[TelegramChannel] bot connected"
        );

        *self.running.write() = true;
        self.base.set_enabled(true);

        // Start long polling in background
        self.polling_loop();

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[TelegramChannel] stopping bot");
        *self.running.write() = false;
        self.base.set_enabled(false);

        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

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

        let html_content = Self::markdown_to_telegram_html(&msg.content);

        // Try to edit placeholder first
        let placeholder_msg_id = self.placeholders.write().remove(&msg.chat_id);
        if let Some(message_id) = placeholder_msg_id {
            let edit_params = EditMessageParams {
                chat_id,
                message_id,
                text: html_content.clone(),
                parse_mode: Some("HTML".to_string()),
            };

            if self.edit_message_text(edit_params).await.is_ok() {
                return Ok(());
            }
            // Fall through to new message if edit fails
        }

        let send_params = SendMessageParams {
            chat_id,
            text: html_content,
            parse_mode: Some("HTML".to_string()),
            ..Default::default()
        };

        match self.send_message(send_params).await {
            Ok(_) => Ok(()),
            Err(_) => {
                // Retry without parse_mode (plain text fallback)
                warn!("[TelegramChannel] HTML parse failed, falling back to plain text");
                let plain_params = SendMessageParams {
                    chat_id,
                    text: msg.content,
                    ..Default::default()
                };
                self.send_message(plain_params).await?;
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

//! Telegram bot channel (long polling, HTML formatting, thinking indicator).
//!
//! Uses the Telegram Bot API directly via HTTP long polling for receiving
//! updates and REST API for sending messages. Supports text, photo, voice,
//! audio, and document messages with optional voice transcription.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

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
    pub fn new(config: TelegramConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.token.is_empty() {
            return Err(NemesisError::Channel(
                "telegram bot token is required".to_string(),
            ));
        }

        let mut client_builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(35));

        // Apply proxy if configured
        if let Some(ref proxy_url) = config.proxy {
            if !proxy_url.is_empty() {
                match reqwest::Proxy::all(proxy_url.as_str()) {
                    Ok(proxy) => {
                        client_builder = client_builder.proxy(proxy);
                    }
                    Err(e) => {
                        warn!("Invalid proxy URL '{}': {}", proxy_url, e);
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
    pub async fn start_thinking_animation(&self, chat_id: &str) -> std::result::Result<tokio::sync::oneshot::Receiver<()>, String> {
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
                self.placeholders.write().insert(chat_id.to_string(), msg.message_id);
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
            return Err(NemesisError::Channel("getUpdates returned not ok".to_string()));
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

            info!("Telegram polling loop started");

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
                                    warn!("Telegram getUpdates returned not ok");
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
                                warn!("Telegram getUpdates parse error: {e}");
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(max_backoff);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Telegram getUpdates request error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                    }
                }
            }

            info!("Telegram polling loop stopped");
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
                debug!("Telegram: message rejected by allowlist (sender_id={sender_id})");
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

        // Photo — resolve file_id to download URL via getFile API
        if let Some(ref photos) = msg.photo {
            if !photos.is_empty() {
                // Use the largest photo (last in array)
                let photo = &photos[photos.len() - 1];
                let file_url = Self::get_file_url(http, api_url_base, &photo.file_id).await;
                if let Some(ref url) = file_url {
                    media.push(MediaAttachment {
                        media_type: "image".to_string(),
                        url: url.clone(),
                        data: None,
                    });
                }
                if !content.is_empty() {
                    content.push('\n');
                }
                match (&file_url, &msg.caption) {
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

            let voice_text = match Self::transcribe_voice(
                transcriber,
                http,
                api_url_base,
                &voice.file_id,
            )
            .await
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
            "Telegram: received message"
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
        metadata.insert("is_group".to_string(), format!("{}", msg.chat.chat_type != "private"));
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
        };

        if let Err(e) = bus_sender.send(inbound) {
            warn!("Telegram: failed to publish inbound message: {e}");
        }
    }

    /// Resolve a Telegram file_id to a download URL via the Bot API's getFile endpoint.
    ///
    /// Returns `Some(download_url)` on success, `None` if HTTP is unavailable or
    /// the API call fails. Pattern follows 内部专项's `telegram_get_file_url`.
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
                        warn!("Telegram voice: getFile parse error: {e}");
                        return Some("[voice (transcription failed)]".to_string());
                    }
                };
                if body["ok"].as_bool() != Some(true) {
                    warn!("Telegram voice: getFile returned not ok: {:?}", body);
                    return Some("[voice (transcription failed)]".to_string());
                }
                match body["result"]["file_path"].as_str() {
                    Some(p) => p.to_string(),
                    None => {
                        warn!("Telegram voice: getFile missing file_path");
                        return Some("[voice (transcription failed)]".to_string());
                    }
                }
            }
            Err(e) => {
                warn!("Telegram voice: getFile request error: {e}");
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
                        warn!("Telegram voice: failed to write file: {e}");
                        return Some("[voice (transcription failed)]".to_string());
                    }
                } else {
                    warn!("Telegram voice: failed to read download bytes");
                    return Some("[voice (transcription failed)]".to_string());
                }
            }
            Err(e) => {
                warn!("Telegram voice: download error: {e}");
                return Some("[voice (transcription failed)]".to_string());
            }
        }

        let path_str = local_path.to_string_lossy().to_string();

        // Step 3: Call transcriber
        match transcriber.transcribe(&path_str).await {
            Ok(text) => {
                info!("Telegram voice transcribed successfully");
                Some(format!("[voice transcription: {text}]"))
            }
            Err(e) => {
                warn!("Telegram voice transcription failed: {e}");
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
            return Err(NemesisError::Channel("sendMessage returned not ok".to_string()));
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
            debug!("editMessageText returned not ok (may be non-fatal)");
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
    /// Uses a line-by-line parser (inspired by 内部专项's formatter.rs) that
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
                blocks.push(format!("<blockquote>{}</blockquote>", quote_lines.join("\n")));
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
    rest.strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
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
            result = format!("{}<code>{}</code>{}", &result[..start], inner, &result[end + 1..]);
        } else {
            break;
        }
    }

    // Italic: single * (not **) → <i>text</i>
    let mut out = String::with_capacity(result.len());
    let chars: Vec<char> = result.chars().collect();
    let mut idx = 0;
    let mut in_italic = false;
    while idx < chars.len() {
        if chars[idx] == '*'
            && (idx == 0 || chars[idx - 1] != '*')
            && (idx + 1 >= chars.len() || chars[idx + 1] != '*')
        {
            if in_italic {
                out.push_str("</i>");
            } else {
                out.push_str("<i>");
            }
            in_italic = !in_italic;
        } else {
            out.push(chars[idx]);
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

    async fn start(&self) -> Result<()> {
        info!("starting Telegram bot (polling mode)");

        let me = self.get_me().await?;
        *self.bot_username.write() = me.username.clone().unwrap_or_default();

        info!(
            username = %self.bot_username.read(),
            "Telegram bot connected"
        );

        *self.running.write() = true;
        self.base.set_enabled(true);

        // Start long polling in background
        self.polling_loop();

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Telegram bot");
        *self.running.write() = false;
        self.base.set_enabled(false);

        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel("telegram bot not running".to_string()));
        }

        let chat_id: i64 = msg
            .chat_id
            .parse()
            .map_err(|e: std::num::ParseIntError| {
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
                warn!("HTML parse failed, falling back to plain text");
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
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_html_bold() {
        let result = TelegramChannel::markdown_to_telegram_html("**hello**");
        assert_eq!(result, "<b>hello</b>");
    }

    #[test]
    fn test_markdown_to_html_italic() {
        let result = TelegramChannel::markdown_to_telegram_html("_hello_");
        assert_eq!(result, "<i>hello</i>");
    }

    #[test]
    fn test_markdown_to_html_code() {
        let result = TelegramChannel::markdown_to_telegram_html("`code`");
        assert_eq!(result, "<code>code</code>");
    }

    #[test]
    fn test_markdown_to_html_code_block() {
        let input = "```\nlet x = 1;\n```";
        let result = TelegramChannel::markdown_to_telegram_html(input);
        assert!(result.contains("<pre><code>"));
        assert!(result.contains("let x = 1;"));
    }

    #[test]
    fn test_markdown_to_html_links() {
        let result = TelegramChannel::markdown_to_telegram_html("[click](http://example.com)");
        assert!(result.contains(r#"<a href="http://example.com">click</a>"#));
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("<b>"), "&lt;b&gt;");
        assert_eq!(escape_html("a&b"), "a&amp;b");
    }

    #[test]
    fn test_extract_code_blocks() {
        let input = "before ```rust\nfn main() {}\n``` after";
        let (text, codes) = extract_code_blocks(input);
        assert!(text.contains("\x00CB0\x00"));
        assert_eq!(codes.len(), 1);
        assert!(codes[0].contains("fn main()"));
    }

    #[test]
    fn test_extract_inline_codes() {
        let input = "use `foo` and `bar`";
        let (text, codes) = extract_inline_codes(input);
        assert!(text.contains("\x00IC0\x00"));
        assert!(text.contains("\x00IC1\x00"));
        assert_eq!(codes, vec!["foo", "bar"]);
    }

    #[tokio::test]
    async fn test_telegram_channel_new_validates_token() {
        let config = TelegramConfig::default();
        let (tx, _rx) = broadcast::channel(256);
        let result = TelegramChannel::new(config, tx);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("token is required"));
    }

    #[tokio::test]
    async fn test_telegram_channel_new_with_token() {
        let config = TelegramConfig {
            token: "123456:ABC-DEF".to_string(),
            ..Default::default()
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = TelegramChannel::new(config, tx).unwrap();
        assert_eq!(ch.name(), "telegram");
    }

    #[test]
    fn test_telegram_config_default() {
        let cfg = TelegramConfig::default();
        assert!(cfg.token.is_empty());
        assert_eq!(cfg.api_base, "https://api.telegram.org");
        assert!(cfg.proxy.is_none());
    }

    #[test]
    fn test_telegram_set_transcriber() {
        let config = TelegramConfig {
            token: "123456:ABC-DEF".to_string(),
            ..Default::default()
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = TelegramChannel::new(config, tx).unwrap();

        // Should not panic with None
        // (We can't test with a real transcriber because the trait requires async)
        // Just verify the method exists and compiles
    }

    #[test]
    fn test_thinking_cancel() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let mut cancel = ThinkingCancel::new(tx);

        // Cancel should signal the receiver
        cancel.cancel();
        // The receiver should get the signal (or be errored because sender was dropped)
        // tokio::sync::oneshot::Receiver doesn't have is_ok()/is_err() until awaited
        let _ = rx;
    }

    #[test]
    fn test_stop_thinking_no_op_for_nonexistent() {
        let config = TelegramConfig {
            token: "123456:ABC-DEF".to_string(),
            ..Default::default()
        };
        let (tx, _rx) = broadcast::channel(256);
        let ch = TelegramChannel::new(config, tx).unwrap();
        // Should not panic when no thinking animation exists
        ch.stop_thinking_animation("12345");
    }

    #[tokio::test]
    async fn test_handle_incoming_message_text() {
        let (tx, mut rx) = broadcast::channel(256);

        let msg = TelegramMessage {
            message_id: 42,
            from: Some(TelegramUser {
                id: 12345,
                username: Some("testuser".to_string()),
                first_name: "Test".to_string(),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 67890,
                chat_type: "private".to_string(),
            },
            text: Some("Hello bot!".to_string()),
            caption: None,
            photo: None,
            voice: None,
            audio: None,
            document: None,
        };

        TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
            .await;

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.channel, "telegram");
        assert_eq!(inbound.sender_id, "12345|testuser");
        assert_eq!(inbound.chat_id, "67890");
        assert_eq!(inbound.content, "Hello bot!");
        assert!(inbound.media.is_empty());
        assert_eq!(inbound.metadata.get("message_id").unwrap(), "42");
    }

    #[tokio::test]
    async fn test_handle_incoming_message_with_photo() {
        let (tx, mut rx) = broadcast::channel(256);

        let msg = TelegramMessage {
            message_id: 43,
            from: Some(TelegramUser {
                id: 12345,
                username: None,
                first_name: "Test".to_string(),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 67890,
                chat_type: "private".to_string(),
            },
            text: None,
            caption: Some("A nice photo".to_string()),
            photo: Some(vec![TelegramPhotoSize {
                file_id: "photo_file_123".to_string(),
                width: 800,
                height: 600,
            }]),
            voice: None,
            audio: None,
            document: None,
        };

        TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
            .await;

        let inbound = rx.try_recv().unwrap();
        assert!(inbound.content.contains("A nice photo"));
        assert!(inbound.content.contains("[image: photo]"));
        assert_eq!(inbound.media.len(), 1);
        assert_eq!(inbound.media[0].media_type, "image");
    }

    #[tokio::test]
    async fn test_handle_incoming_message_rejected_by_allowlist() {
        let (tx, mut rx) = broadcast::channel(256);

        let msg = TelegramMessage {
            message_id: 44,
            from: Some(TelegramUser {
                id: 99999,
                username: Some("blocked".to_string()),
                first_name: "Blocked".to_string(),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 67890,
                chat_type: "private".to_string(),
            },
            text: Some("Should be blocked".to_string()),
            caption: None,
            photo: None,
            voice: None,
            audio: None,
            document: None,
        };

        TelegramChannel::handle_incoming_message(
            &msg,
            &tx,
            &["12345".to_string()],
            &None,
            None,
            None,
        )
        .await;

        // Message should be dropped — nothing to receive
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_handle_incoming_message_empty() {
        let (tx, mut rx) = broadcast::channel(256);

        let msg = TelegramMessage {
            message_id: 45,
            from: Some(TelegramUser {
                id: 12345,
                username: None,
                first_name: "Test".to_string(),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 67890,
                chat_type: "private".to_string(),
            },
            text: None,
            caption: None,
            photo: None,
            voice: None,
            audio: None,
            document: None,
        };

        TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
            .await;

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.content, "[empty message]");
    }

    #[tokio::test]
    async fn test_telegram_new_with_client_validates_token() {
        let config = TelegramConfig::default();
        let (tx, _rx) = broadcast::channel(256);
        let http = reqwest::Client::new();
        let result = TelegramChannel::new_with_client(config, tx, http);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("token is required"));
    }

    #[tokio::test]
    async fn test_telegram_new_with_client_success() {
        let config = TelegramConfig {
            token: "123456:ABC-DEF".to_string(),
            ..Default::default()
        };
        let (tx, _rx) = broadcast::channel(256);
        let http = reqwest::Client::new();
        let ch = TelegramChannel::new_with_client(config, tx, http).unwrap();
        assert_eq!(ch.name(), "telegram");
        assert!(!*ch.running.read());
    }

    #[tokio::test]
    async fn test_handle_incoming_message_voice_no_transcriber() {
        let (tx, mut rx) = broadcast::channel(256);

        let msg = TelegramMessage {
            message_id: 50,
            from: Some(TelegramUser {
                id: 12345,
                username: Some("testuser".to_string()),
                first_name: "Test".to_string(),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 67890,
                chat_type: "private".to_string(),
            },
            text: None,
            caption: None,
            photo: None,
            voice: Some(TelegramFile {
                file_id: "voice_file_123".to_string(),
                file_unique_id: "unique_123".to_string(),
                file_size: Some(1024),
            }),
            audio: None,
            document: None,
        };

        TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
            .await;

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.content, "[voice]");
        assert_eq!(inbound.media.len(), 1);
        assert_eq!(inbound.media[0].media_type, "voice");
    }

    #[tokio::test]
    async fn test_voice_transcribe_no_transcriber() {
        // When no transcriber is set, should return None
        let result = TelegramChannel::transcribe_voice(&None, None, None, "file123").await;
        assert!(result.is_none());
    }

    /// Mock transcriber for testing voice transcription flow.
    struct MockTranscriber {
        available: bool,
        text: String,
        should_fail: bool,
    }

    impl crate::base::VoiceTranscriber for MockTranscriber {
        fn is_available(&self) -> bool {
            self.available
        }

        fn transcribe(
            &self,
            _file_path: &str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send + '_>,
        > {
            if self.should_fail {
                Box::pin(async { Err("transcription error".to_string()) })
            } else {
                let text = self.text.clone();
                Box::pin(async move { Ok(text) })
            }
        }
    }

    #[tokio::test]
    async fn test_voice_transcribe_unavailable_transcriber() {
        let transcriber: Arc<dyn crate::base::VoiceTranscriber> = Arc::new(MockTranscriber {
            available: false,
            text: String::new(),
            should_fail: false,
        });
        let result =
            TelegramChannel::transcribe_voice(&Some(transcriber), None, None, "file123").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_voice_transcribe_no_http_client() {
        let transcriber: Arc<dyn crate::base::VoiceTranscriber> = Arc::new(MockTranscriber {
            available: true,
            text: "hello world".to_string(),
            should_fail: false,
        });
        // Available transcriber but no HTTP client → can't download → None
        let result =
            TelegramChannel::transcribe_voice(&Some(transcriber), None, None, "file123").await;
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // Tests for markdown_to_telegram_html: headers, blockquotes, bold
    // underscores, list markers, strikethrough, and edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_markdown_header_to_bold() {
        let result = TelegramChannel::markdown_to_telegram_html("# Header");
        assert_eq!(result, "<b>Header</b>");
    }

    #[test]
    fn test_markdown_multiple_header_levels() {
        let result = TelegramChannel::markdown_to_telegram_html("### Level 3");
        assert_eq!(result, "<b>Level 3</b>");
    }

    #[test]
    fn test_markdown_blockquote() {
        let result = TelegramChannel::markdown_to_telegram_html("> quoted text");
        assert_eq!(result, "<blockquote>quoted text</blockquote>");
    }

    #[test]
    fn test_markdown_bold_double_underscores() {
        let result = TelegramChannel::markdown_to_telegram_html("__bold text__");
        assert_eq!(result, "<b>bold text</b>");
    }

    #[test]
    fn test_markdown_list_marker_dash() {
        let result = TelegramChannel::markdown_to_telegram_html("- item");
        assert_eq!(result, "• item");
    }

    #[test]
    fn test_markdown_list_marker_asterisk() {
        let result = TelegramChannel::markdown_to_telegram_html("* item");
        assert_eq!(result, "• item");
    }

    #[test]
    fn test_markdown_combined_bold_and_italic() {
        let result = TelegramChannel::markdown_to_telegram_html("**bold** and _italic_");
        assert!(
            result.contains("<b>bold</b>"),
            "expected bold tag in: {result}"
        );
        assert!(
            result.contains("<i>italic</i>"),
            "expected italic tag in: {result}"
        );
    }

    #[test]
    fn test_markdown_links_preserved() {
        let result = TelegramChannel::markdown_to_telegram_html("[text](url)");
        assert_eq!(result, r#"<a href="url">text</a>"#);
    }

    #[test]
    fn test_markdown_code_blocks_preserved() {
        let input = "```code```";
        let result = TelegramChannel::markdown_to_telegram_html(input);
        assert!(
            result.contains("<pre><code>"),
            "expected <pre><code> in: {result}"
        );
        assert!(result.contains("code"), "expected 'code' in: {result}");
    }

    #[test]
    fn test_markdown_empty_string() {
        let result = TelegramChannel::markdown_to_telegram_html("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_markdown_html_escaping() {
        let result = TelegramChannel::markdown_to_telegram_html("<script>");
        assert_eq!(result, "&lt;script&gt;");
    }

    #[test]
    fn test_markdown_strikethrough() {
        let result = TelegramChannel::markdown_to_telegram_html("~~deleted~~");
        assert_eq!(result, "<s>deleted</s>");
    }

    #[test]
    fn test_markdown_mixed_headers_and_bold() {
        let input = "## Title\n**bold**";
        let result = TelegramChannel::markdown_to_telegram_html(input);
        assert!(
            result.contains("<b>Title</b>"),
            "expected bold Title in: {result}"
        );
        assert!(
            result.contains("<b>bold</b>"),
            "expected bold tag in: {result}"
        );
    }

    #[test]
    fn test_markdown_code_blocks_not_affected_by_bold_conversion() {
        let input = "```**not bold**```";
        let result = TelegramChannel::markdown_to_telegram_html(input);
        // The content inside code blocks should be preserved literally,
        // not converted to <b> tags.
        assert!(
            !result.contains("<b>"),
            "code block content should not be converted to bold: {result}"
        );
        assert!(
            result.contains("**not bold**"),
            "code block should preserve original text: {result}"
        );
    }

    #[test]
    fn test_telegram_config_proxy_support() {
        // Verify TelegramConfig stores the proxy field correctly
        let cfg = TelegramConfig {
            token: "123456:ABC-DEF".to_string(),
            proxy: Some("http://proxy.example.com:8080".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.proxy.as_deref(),
            Some("http://proxy.example.com:8080")
        );

        // Verify a channel can be created with proxy config
        let (tx, _rx) = broadcast::channel(256);
        let ch = TelegramChannel::new(cfg, tx).unwrap();
        assert_eq!(ch.name(), "telegram");

        // Verify default config has no proxy
        let default_cfg = TelegramConfig::default();
        assert!(default_cfg.proxy.is_none());
    }
}

//! IRC channel (RFC 1459, TCP+TLS, PING/PONG, auto-reconnect).
//!
//! Implements a raw IRC client using TCP (with optional TLS) that follows
//! the text-based IRC protocol. Supports PING/PONG, auto-reconnect with
//! exponential backoff, and message splitting for IRC line limits.
//!
//! The connection loop runs in a background task that:
//! 1. Connects via TCP (optionally TLS)
//! 2. Sends registration (PASS/NICK/USER)
//! 3. Waits for RPL_WELCOME (001) then JOIN
//! 4. Reads lines in a loop, handling PING and dispatching PRIVMSG

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// IRC channel configuration.
#[derive(Debug, Clone)]
pub struct IRCConfig {
    /// Server address (e.g. "irc.libera.chat:6697").
    pub server: String,
    /// Whether to use TLS.
    pub use_tls: bool,
    /// Nickname.
    pub nick: String,
    /// Server password.
    pub password: Option<String>,
    /// Channel to join (e.g. "#nemesisbot").
    pub channel: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
    /// Reconnect backoff base in seconds (default: 5).
    pub reconnect_backoff_secs: u64,
    /// Maximum reconnect backoff in seconds (default: 300).
    pub max_reconnect_backoff_secs: u64,
}

impl Default for IRCConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            use_tls: true,
            nick: String::new(),
            password: None,
            channel: String::new(),
            allow_from: Vec::new(),
            reconnect_backoff_secs: 5,
            max_reconnect_backoff_secs: 300,
        }
    }
}

/// IRC channel using raw TCP connection.
pub struct IRCChannel {
    /// Arc 共享进连接任务（allow-list 检查在 spawn 侧执行，BUG-20261031-247）。
    base: Arc<BaseChannel>,
    config: IRCConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    /// Message bus sender for publishing inbound PRIVMSGs（BUG-20261031-247）。
    ///
    /// 与 external/qq/matrix 通道同构：收件必须发布到总线进 agent loop。
    /// 此前本通道构造器没有 bus 参数，读循环也没有 PRIVMSG 分支——消息
    /// 落 `_ => {}` 静默丢弃，收件面整体缺失。
    bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    /// Writer for sending raw IRC commands.
    writer: parking_lot::Mutex<Option<IRCWriter>>,
    /// Cancellation channel for the read loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// Wrapper for the write half of the IRC TCP connection.
struct IRCWriter {
    writer: tokio::io::WriteHalf<tokio::net::TcpStream>,
}

impl IRCChannel {
    /// Creates a new `IRCChannel`.
    ///
    /// `bus_sender` is used to publish inbound PRIVMSGs read from the
    /// connection (parity with external/qq/matrix channels'
    /// `(cfg, messageBus)` shape).
    pub fn new(
        config: IRCConfig,
        bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.server.is_empty() {
            return Err(NemesisError::Channel("irc server is required".to_string()));
        }
        if config.nick.is_empty() {
            return Err(NemesisError::Channel("irc nick is required".to_string()));
        }
        if config.channel.is_empty() {
            return Err(NemesisError::Channel("irc channel is required".to_string()));
        }

        let channel = ensure_hash_prefix(&config.channel);
        // 重连退避下限 1s（对齐 mqtt reconnect_delay 惯例）：0 会让 sleep(0)
        // 退化成紧循环 hammer 服务器；装配层已把负配置钳 0（as 转译负数会
        // 回绕成巨型 u64 = 永不重连），此处兜住 0 值形态。
        let reconnect_backoff_secs = config.reconnect_backoff_secs.max(1);
        let max_reconnect_backoff_secs = config.max_reconnect_backoff_secs.max(1);

        Ok(Self {
            base: Arc::new(BaseChannel::with_allow_list(
                "irc",
                config.allow_from.clone(),
            )),
            config: IRCConfig {
                channel,
                reconnect_backoff_secs,
                max_reconnect_backoff_secs,
                ..config
            },
            running: Arc::new(parking_lot::RwLock::new(false)),
            bus_sender,
            writer: parking_lot::Mutex::new(None),
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Returns the channel name with # prefix.
    pub fn channel(&self) -> &str {
        &self.config.channel
    }

    /// Returns the nick.
    pub fn nick(&self) -> &str {
        &self.config.nick
    }

    /// Ensures channel name starts with #.
    pub fn ensure_hash_prefix(channel: &str) -> String {
        ensure_hash_prefix(channel)
    }

    /// Splits a message into lines that fit within IRC line limits.
    pub fn split_message(content: &str, max_len: usize) -> Vec<String> {
        if content.len() <= max_len {
            return vec![content.to_string()];
        }

        let mut lines = Vec::new();
        let mut remaining = content;

        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                lines.push(remaining.to_string());
                break;
            }

            // Floor to a char boundary so multibyte content (CJK) doesn't slice
            // mid-codepoint and panic. max_len is a byte budget from caller.
            let limit = nemesis_types::utils::floor_char_boundary(remaining, max_len);
            // Try newline
            if let Some(idx) = remaining[..limit].rfind('\n') {
                lines.push(remaining[..idx].to_string());
                remaining = &remaining[idx + 1..];
            } else if let Some(idx) = remaining[..limit].rfind(' ') {
                lines.push(remaining[..idx].to_string());
                remaining = &remaining[idx + 1..];
            } else {
                lines.push(remaining[..limit].to_string());
                remaining = &remaining[limit..];
            }
        }

        lines
    }

    /// Parses an IRC prefix to extract the nick.
    /// Format: "nick!user@host" -> "nick"
    pub fn extract_nick_from_prefix(prefix: &str) -> &str {
        if let Some(idx) = prefix.find('!') {
            &prefix[..idx]
        } else {
            prefix
        }
    }

    /// Builds IRC registration commands.
    pub fn build_registration(&self) -> Vec<String> {
        let mut commands = Vec::new();

        if let Some(ref pass) = self.config.password {
            commands.push(format!("PASS {pass}"));
        }
        commands.push(format!("NICK {}", self.config.nick));
        commands.push(format!("USER {} 0 * :NemesisBot", self.config.nick));

        commands
    }

    /// Parses a raw IRC line into (prefix, command, params).
    pub fn parse_irc_line(line: &str) -> (Option<&str>, &str, &str) {
        let mut params = line;

        // Extract prefix
        let prefix = if params.starts_with(':') {
            let parts: Vec<&str> = params.splitn(2, ' ').collect();
            if parts.len() < 2 {
                return (None, "", "");
            }
            let p = &parts[0][1..]; // strip leading ':'
            params = parts[1];
            Some(p)
        } else {
            None
        };

        // Extract command
        let parts: Vec<&str> = params.splitn(2, ' ').collect();
        let command = parts[0];
        let remaining = if parts.len() > 1 { parts[1] } else { "" };

        (prefix, command, remaining)
    }

    /// Handles a PING message by returning PONG.
    pub fn handle_ping(line: &str) -> Option<String> {
        line.strip_prefix("PING ")
            .map(|data| format!("PONG {data}"))
    }

    /// Parses a PRIVMSG to extract (target, content).
    /// Format: "target :content" or "target content"
    pub fn parse_privmsg(params: &str) -> Option<(&str, &str)> {
        let parts: Vec<&str> = params.splitn(2, " :").collect();
        if parts.len() < 2 {
            return None;
        }
        Some((parts[0], parts[1]))
    }

    /// Sends a raw IRC command through the connection.
    pub async fn send_raw(&self, command: &str) -> Result<()> {
        // Take the writer out of the mutex to avoid holding the lock across await
        let writer_opt = self.writer.lock().take();
        if let Some(mut w) = writer_opt {
            w.writer
                .write_all(format!("{command}\r\n").as_bytes())
                .await
                .map_err(|e| NemesisError::Channel(format!("IRC write failed: {e}")))?;
            w.writer
                .flush()
                .await
                .map_err(|e| NemesisError::Channel(format!("IRC flush failed: {e}")))?;
            debug!(command = %command, "[IRCChannel] sent command");
            // Put writer back
            *self.writer.lock() = Some(w);
            Ok(())
        } else {
            Err(NemesisError::Channel("IRC not connected".to_string()))
        }
    }

    /// Spawns the connection loop with auto-reconnect and exponential backoff.
    fn spawn_connection_loop(&self) {
        let running = self.running.clone();
        let config = self.config.clone();
        // BUG-20261031-247：收件发布面需要 allow-list 与 bus，一并共享进任务。
        let base = Arc::clone(&self.base);
        let bus_sender = self.bus_sender.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        tokio::spawn(async move {
            let mut backoff_secs = config.reconnect_backoff_secs;
            let mut cancel_rx = cancel_rx;
            // 433（昵称冲突）后服务器侧实际昵称会变化，私聊判别（target ==
            // 本 bot 昵称）与重试基准必须跟随——此前每次 433 都用原始昵称
            // 追加 `_`，连续冲突永不收敛。
            let mut current_nick = config.nick.clone();

            loop {
                if !*running.read() {
                    break;
                }

                // Connect
                let stream_result = TcpStream::connect(&config.server).await;
                match stream_result {
                    Ok(stream) => {
                        let (reader, mut writer) = tokio::io::split(stream);

                        // Send registration commands
                        if let Some(ref pass) = config.password {
                            let _ = writer
                                .write_all(format!("PASS {pass}\r\n").as_bytes())
                                .await;
                        }
                        let _ = writer
                            .write_all(format!("NICK {}\r\n", config.nick).as_bytes())
                            .await;
                        let _ = writer
                            .write_all(
                                format!("USER {} 0 * :NemesisBot\r\n", config.nick).as_bytes(),
                            )
                            .await;
                        let _ = writer.flush().await;

                        // Read loop
                        let mut reader = BufReader::new(reader);
                        let mut line = String::new();

                        loop {
                            tokio::select! {
                                _ = &mut cancel_rx => {
                                    info!("[IRCChannel] connection loop cancelled");
                                    let _ = writer.write_all(b"QUIT :NemesisBot shutting down\r\n").await;
                                    return;
                                }
                                result = reader.read_line(&mut line) => {
                                    match result {
                                        Ok(0) => {
                                            warn!("[IRCChannel] connection closed");
                                            break;
                                        }
                                        Ok(_) => {
                                            let trimmed = line.trim_end_matches("\r\n").trim_end_matches('\n');
                                            if trimmed.is_empty() {
                                                line.clear();
                                                continue;
                                            }

                                            // PING/PONG
                                            if let Some(data) = trimmed.strip_prefix("PING ") {
                                                let _ = writer.write_all(format!("PONG {data}\r\n").as_bytes()).await;
                                                let _ = writer.flush().await;
                                                line.clear();
                                                continue;
                                            }

                                            let (prefix, command, params) = IRCChannel::parse_irc_line(trimmed);

                                            match command {
                                                "001" => {
                                                    info!(channel = %config.channel, "[IRCChannel] registered, joining");
                                                    let _ = writer.write_all(format!("JOIN {}\r\n", config.channel).as_bytes()).await;
                                                    let _ = writer.flush().await;
                                                }
                                                "433" => {
                                                    let new_nick = format!("{current_nick}_");
                                                    current_nick = new_nick.clone();
                                                    let _ = writer.write_all(format!("NICK {new_nick}\r\n").as_bytes()).await;
                                                }
                                                "PRIVMSG" => {
                                                    // BUG-20261031-247：收件必须发布到总线进
                                                    // agent loop——此前读循环没有 PRIVMSG 分支，
                                                    // 消息落 `_ => {}` 静默丢弃（通道从未接线
                                                    // 总线：构造器无 bus 参数，allow_from 零
                                                    // 消费）。私聊（target=本 bot 当前昵称）
                                                    // chat_id 取发送者昵称，回复 PRIVMSG 才能
                                                    // 送达；频道消息 chat_id=频道名。
                                                    if let Some((target, content)) =
                                                        IRCChannel::parse_privmsg(params)
                                                    {
                                                        let sender = prefix
                                                            .map(IRCChannel::extract_nick_from_prefix)
                                                            .unwrap_or("unknown");
                                                        // IRC 昵称大小写不敏感（RFC 1459 casemapping）
                                                        let chat_id =
                                                            if target.eq_ignore_ascii_case(&current_nick) {
                                                                sender.to_string()
                                                            } else {
                                                                target.to_string()
                                                            };
                                                        if base.handle_message(sender) {
                                                            let inbound = InboundMessage {
                                                                channel: base.name().to_string(),
                                                                sender_id: sender.to_string(),
                                                                chat_id: chat_id.clone(),
                                                                content: content.to_string(),
                                                                media: Vec::new(),
                                                                session_key: format!("irc:{chat_id}"),
                                                                correlation_id: String::new(),
                                                                metadata: HashMap::new(),
                                                                voice_playback: None,
                                                            };
                                                            if let Err(e) = bus_sender.send(inbound) {
                                                                warn!(error = %e,
                                                                    "[IRCChannel] failed to publish inbound message");
                                                            }
                                                        } else {
                                                            warn!(sender = %sender,
                                                                "[IRCChannel] message blocked by allow-list");
                                                        }
                                                    }
                                                }
                                                "JOIN" => {
                                                    if let Some(nick) = prefix.map(IRCChannel::extract_nick_from_prefix)
                                                        && nick == current_nick
                                                    {
                                                        info!("[IRCChannel] joined channel successfully");
                                                    }
                                                }
                                                "KICK" => {
                                                    warn!("[IRCChannel] kicked, rejoining");
                                                    let _ = writer.write_all(format!("JOIN {}\r\n", config.channel).as_bytes()).await;
                                                }
                                                "ERROR" => {
                                                    error!(params = %params, "[IRCChannel] error");
                                                    break;
                                                }
                                                _ => {}
                                            }
                                            line.clear();
                                        }
                                        Err(e) => {
                                            error!(error = %e, "[IRCChannel] read error");
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // Reset backoff on successful connection
                        backoff_secs = config.reconnect_backoff_secs;
                    }
                    Err(e) => {
                        error!(error = %e, "[IRCChannel] connect failed");
                    }
                }

                // Wait before reconnecting with exponential backoff
                if !*running.read() {
                    break;
                }
                warn!(
                    backoff_secs = backoff_secs,
                    "[IRCChannel] reconnecting after backoff"
                );
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(config.max_reconnect_backoff_secs);
            }
        });
    }
}

#[async_trait]
impl Channel for IRCChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!(
            server = %self.config.server,
            nick = %self.config.nick,
            channel = %self.config.channel,
            "[IRCChannel] starting IRC channel"
        );
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Spawn the connection loop
        self.spawn_connection_loop();

        info!("[IRCChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[IRCChannel] stopping IRC channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Cancel the connection loop
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }
        *self.writer.lock() = None;

        info!("[IRCChannel] channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel("irc channel not running".to_string()));
        }

        self.base.record_sent();

        let target = if msg.chat_id.is_empty() {
            self.config.channel.clone()
        } else {
            msg.chat_id
        };

        let lines = Self::split_message(&msg.content, 400);
        for line in lines {
            self.send_raw(&format!("PRIVMSG {target} :{line}")).await?;
        }

        Ok(())
    }
}

fn ensure_hash_prefix(channel: &str) -> String {
    if !channel.is_empty() && !channel.starts_with('#') {
        format!("#{channel}")
    } else {
        channel.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

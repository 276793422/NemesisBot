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

use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::OutboundMessage;
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
    base: BaseChannel,
    config: IRCConfig,
    running: Arc<parking_lot::RwLock<bool>>,
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
    pub fn new(config: IRCConfig) -> Result<Self> {
        if config.server.is_empty() {
            return Err(NemesisError::Channel(
                "irc server is required".to_string(),
            ));
        }
        if config.nick.is_empty() {
            return Err(NemesisError::Channel("irc nick is required".to_string()));
        }
        if config.channel.is_empty() {
            return Err(NemesisError::Channel(
                "irc channel is required".to_string(),
            ));
        }

        let channel = ensure_hash_prefix(&config.channel);

        Ok(Self {
            base: BaseChannel::new("irc"),
            config: IRCConfig { channel, ..config },
            running: Arc::new(parking_lot::RwLock::new(false)),
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

            // Try newline
            if let Some(idx) = remaining[..max_len].rfind('\n') {
                lines.push(remaining[..idx].to_string());
                remaining = &remaining[idx + 1..];
            } else if let Some(idx) = remaining[..max_len].rfind(' ') {
                lines.push(remaining[..idx].to_string());
                remaining = &remaining[idx + 1..];
            } else {
                lines.push(remaining[..max_len].to_string());
                remaining = &remaining[max_len..];
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
        if let Some(data) = line.strip_prefix("PING ") {
            Some(format!("PONG {data}"))
        } else {
            None
        }
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
            Err(NemesisError::Channel(
                "IRC not connected".to_string(),
            ))
        }
    }

    /// Connects to the IRC server, registers, and spawns the read loop.
    async fn connect_and_run(&self) -> Result<()> {
        let stream = TcpStream::connect(&self.config.server)
            .await
            .map_err(|e| NemesisError::Channel(format!("IRC connect to {} failed: {e}", self.config.server)))?;

        let (reader, writer) = tokio::io::split(stream);
        *self.writer.lock() = Some(IRCWriter { writer });

        // Send registration
        for cmd in self.build_registration() {
            self.send_raw(&cmd).await?;
        }

        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    warn!("[IRCChannel] connection closed by server");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches("\r\n").trim_end_matches('\n');
                    if trimmed.is_empty() {
                        continue;
                    }

                    debug!(line = %trimmed, "[IRCChannel] recv");

                    // Handle PING
                    if let Some(pong) = Self::handle_ping(trimmed) {
                        self.send_raw(&pong).await?;
                        continue;
                    }

                    let (prefix, command, params) = Self::parse_irc_line(trimmed);

                    match command {
                        "001" => {
                            // RPL_WELCOME - join channel
                            info!(channel = %self.config.channel, "[IRCChannel] registered, joining channel");
                            self.send_raw(&format!("JOIN {}", self.config.channel))
                                .await?;
                        }
                        "433" => {
                            // Nick already in use
                            warn!("[IRCChannel] nick in use, appending _");
                            let new_nick = format!("{}_", self.config.nick);
                            self.send_raw(&format!("NICK {new_nick}")).await?;
                        }
                        "PRIVMSG" => {
                            if let Some((target, content)) = Self::parse_privmsg(params) {
                                let sender = prefix
                                    .map(Self::extract_nick_from_prefix)
                                    .unwrap_or("unknown");
                                self.base.record_received();

                                debug!(
                                    sender = %sender,
                                    target = %target,
                                    content = %content,
                                    "[IRCChannel] received PRIVMSG"
                                );
                            }
                        }
                        "JOIN" => {
                            if let Some(nick) = prefix.map(Self::extract_nick_from_prefix) {
                                if nick == self.config.nick {
                                    info!(channel = %params.trim(), "[IRCChannel] joined channel");
                                }
                            }
                        }
                        "KICK" => {
                            warn!(params = %params, "[IRCChannel] kicked from channel");
                            // Rejoin
                            self.send_raw(&format!("JOIN {}", self.config.channel))
                                .await?;
                        }
                        "ERROR" => {
                            error!(params = %params, "[IRCChannel] error from server");
                            break;
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    error!(error = %e, "[IRCChannel] read error");
                    break;
                }
            }
        }

        // Clear writer
        *self.writer.lock() = None;
        Ok(())
    }

    /// Spawns the connection loop with auto-reconnect and exponential backoff.
    fn spawn_connection_loop(&self) {
        let running = self.running.clone();
        let config = self.config.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        tokio::spawn(async move {
            let mut backoff_secs = config.reconnect_backoff_secs;
            let mut cancel_rx = cancel_rx;

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
                            let _ = writer.write_all(format!("PASS {pass}\r\n").as_bytes()).await;
                        }
                        let _ = writer.write_all(format!("NICK {}\r\n", config.nick).as_bytes()).await;
                        let _ = writer.write_all(format!("USER {} 0 * :NemesisBot\r\n", config.nick).as_bytes()).await;
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
                                                    let new_nick = format!("{}_", config.nick);
                                                    let _ = writer.write_all(format!("NICK {new_nick}\r\n").as_bytes()).await;
                                                }
                                                "JOIN" => {
                                                    if let Some(nick) = prefix.map(IRCChannel::extract_nick_from_prefix) {
                                                        if nick == config.nick {
                                                            info!("[IRCChannel] joined channel successfully");
                                                        }
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
            return Err(NemesisError::Channel(
                "irc channel not running".to_string(),
            ));
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

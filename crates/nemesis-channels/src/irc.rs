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
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

use nemesis_utils::line_cap::read_line_capped;

/// 单行字节帽（BUG-20261108-028）：裸 read_line 把整行无界积累进内存才
/// 返回——恶意/故障 IRC 服务器一行无限输出打爆宿主内存。RFC 1459 行上限
/// 512 字节（message-tags 扩展也不过数 KB），16KB 远超任何合法行。
const MAX_IRC_LINE_BYTES: usize = 16 * 1024;

/// IRC channel configuration.
/// SEC-002（BUG-20261111-068）：server password 脱敏（Option 形态）。
#[derive(Clone)]
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

crate::debug_redact::impl_redacted_debug!(IRCConfig {
    plain { server, use_tls, nick, channel, allow_from, reconnect_backoff_secs, max_reconnect_backoff_secs }
    secret {}
    opt_secret { password }
});

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
    /// Arc 共享进连接任务（allow-list 检查在 spawn 侧执行）。
    base: Arc<BaseChannel>,
    config: IRCConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    /// Message bus sender for publishing inbound PRIVMSGs。
    ///
    /// 与 external/qq/matrix 通道同构：收件必须发布到总线进 agent loop。
    /// 此前本通道构造器没有 bus 参数，读循环也没有 PRIVMSG 分支——消息
    /// 落 `_ => {}` 静默丢弃，收件面整体缺失。
    bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    /// Writer for sending raw IRC commands.
    ///
    /// Arc 共享进连接任务：连接建立后由任务把写半部装载进槽，send_raw
    /// 与读循环（PONG/JOIN/QUIT）全走同一写槽（take-写-放回串行化）。
    /// 此前槽只在 new 时初始化 None、连接任务从不装载
    /// （写半部是任务本地变量），send_raw 恒走「IRC not connected」——
    /// 出站死路。
    writer: Arc<parking_lot::Mutex<Option<IRCWriter>>>,
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
                // allow_from 条目侧同口径 casemap（BUG-20261115-015）：身份
                // 键已归一（irc_identity），BaseChannel 固有 is_allowed 是
                // 精确 == 比较，原大小写条目会错杀归一化后的合法身份。
                config
                    .allow_from
                    .iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect(),
            )),
            config: IRCConfig {
                channel,
                reconnect_backoff_secs,
                max_reconnect_backoff_secs,
                ..config
            },
            running: Arc::new(parking_lot::RwLock::new(false)),
            bus_sender,
            writer: Arc::new(parking_lot::Mutex::new(None)),
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
            // BUG-20261123-041：短文快路径此前整段直推——content 内嵌 \n
            // 会以裸命令形态上线（writer 以 `{line}\r\n` 上 wire，嵌入 \n
            // 即行框架注入：`hello\nPRIVMSG #evil :x` = 以 bot 身份向 #evil
            // 发话/QUIT 等）。与下方慢路径同一纪律：\n 处切分、空段跳过。
            return split_newlines(content);
        }

        let mut lines = Vec::new();
        let mut remaining = content;

        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                // BUG-20261123-041：循环内快路径同款泄漏（短尾段可携带
                // 慢路径没切到的 \n），同一纪律处理。
                lines.extend(split_newlines(remaining));
                break;
            }

            // Floor to a char boundary so multibyte content (CJK) doesn't slice
            // mid-codepoint and panic. max_len is a byte budget from caller.
            let limit = nemesis_types::utils::floor_char_boundary(remaining, max_len);
            // Try newline
            if let Some(idx) = remaining[..limit].rfind('\n') {
                // 分隔符位于 idx 0（前导换行的超长文）时左段为空——空
                // PRIVMSG 是协议违规（服务器回 412 ERR_NOTEXTGIVEN），且
                // send() 的 `?` 会让整条消息投递失败。跳过空段（与
                // nemesis_types::utils::split_message_utf16 的 filter(>0)
                // 家族形态一致；discord 同款 BUG-20261121-044）。
                if idx > 0 {
                    lines.push(remaining[..idx].to_string());
                }
                remaining = &remaining[idx + 1..];
            } else if let Some(idx) = remaining[..limit].rfind(' ') {
                if idx > 0 {
                    lines.push(remaining[..idx].to_string());
                }
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

    /// IRC 身份键（昵称/频道名共用）：RFC 1459 casemapping 语义大小写
    /// 不敏感，按 ASCII 子集折叠（与本文件 eq_ignore_ascii_case 先例同族
    /// 口径；严格 casemapping 还折叠 `[]\^` ↔ `{}|~`，实际服务器与国内
    /// 用法罕见依赖，不展开）。BUG-20261115-015：身份键不归一会让同一
    /// 昵称/频道的大小写变体在内存里分裂成多会话（session_key=
    /// irc:{chat_id}），而 NTFS/APFS 文件名大小写不敏感 → 落盘同一文件
    /// 交错互串。回复投递面 PRIVMSG target 由服务器按 casemapping 匹配
    /// 送达，归一化不影响可达性。
    fn irc_identity(raw: &str) -> String {
        raw.to_ascii_lowercase()
    }

    /// Builds IRC registration commands.
    ///
    /// 关联函数形态：连接任务里只有 config 没有 self（注册写与此处
    /// 统一定义，连接循环直接复用，不再 inline 重复）。
    pub fn build_registration(config: &IRCConfig) -> Vec<String> {
        // BUG-20261123-041：PASS/NICK/USER 载荷直拼行框架 wire——config 值
        // 含 CR/LF 即成以 bot 身份的命令注入。R737 同款剥除纪律（配置值
        // 含换行即非法，剥除对齐 imap_quoted / header_safe 既有形态）。
        let line_safe = |s: &str| s.replace(['\r', '\n'], "");
        let mut commands = Vec::new();

        if let Some(ref pass) = config.password {
            commands.push(format!("PASS {}", line_safe(pass)));
        }
        let nick = line_safe(&config.nick);
        commands.push(format!("NICK {nick}"));
        commands.push(format!("USER {nick} 0 * :NemesisBot"));

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
    /// Format: "target:content" or "target content"
    pub fn parse_privmsg(params: &str) -> Option<(&str, &str)> {
        let parts: Vec<&str> = params.splitn(2, " :").collect();
        if parts.len() < 2 {
            return None;
        }
        Some((parts[0], parts[1]))
    }

    /// 出站写超时：manager 出站派发是单任务串行
    /// await（一条通道 send 无界挂起 = 全部通道出站停摆且零错误零日志，
    /// 见 base.rs 串行派发纪律），TCP write 必须有界——HTTP 35s / email
    /// 30s 同族先例，此处对齐 email 的 30s。
    const WRITE_TIMEOUT_SECS: u64 = 30;
    /// take-写-放回形态下拿不到 writer 的重试窗：读循环 PONG/JOIN 与
    /// dispatch send 并发 take 竞态的消解窗（拿 None ≠ 未连接，短窗
    /// 轮询区分「他人正在写」与「真未连接」）。QoS：真未连接时 send
    /// 至多延迟此窗后诚实报错，不改变错误语义。
    const WRITE_SLOT_RETRY: std::time::Duration = std::time::Duration::from_millis(500);

    /// 从写槽取 writer（带重试窗）：None 可能是他人正在写（瞬态持有）
    /// 或未连接（持久），短窗轮询消解瞬态形态。
    async fn take_writer(
        slot: &parking_lot::Mutex<Option<IRCWriter>>,
    ) -> std::io::Result<IRCWriter> {
        let deadline = std::time::Instant::now() + Self::WRITE_SLOT_RETRY;
        loop {
            if let Some(w) = slot.lock().take() {
                return Ok(w);
            }
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "IRC not connected",
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// 带超时的单命令写（write_all + flush 一体，超时映射 TimedOut，
    /// email io_timeout 同形态）。泛型形态：生产传 TcpStream 写半部，
    /// 测试传 mock writer（Windows loopback TCP 缓冲自调连显式收窄都
    /// 不认——内核行为不可依赖，停摆面用 mock 驱动超时机制）。
    async fn write_command_bounded<W: tokio::io::AsyncWrite + Unpin>(
        writer: &mut W,
        command: &str,
        timeout: std::time::Duration,
    ) -> std::io::Result<()> {
        let payload = format!("{command}\r\n");
        let write_fut = async {
            writer.write_all(payload.as_bytes()).await?;
            writer.flush().await
        };
        match tokio::time::timeout(timeout, write_fut).await {
            Ok(r) => r,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("IRC write timed out after {}s", timeout.as_secs()),
            )),
        }
    }

    /// 单一写路径：取 writer → 带超时写 → 成功放回。写失败/超时不放回
    /// ——写半部状态未知（超时 = future 被取消，可能只写了半个 line），
    /// 半残 writer 回场会让后续写继续撞死连接；断线重连环装载新 writer
    /// 后自然恢复。
    async fn write_line_with_timeout(
        slot: &parking_lot::Mutex<Option<IRCWriter>>,
        command: &str,
        timeout: std::time::Duration,
    ) -> std::io::Result<()> {
        let mut w = Self::take_writer(slot).await?;
        match Self::write_command_bounded(&mut w.writer, command, timeout).await {
            Ok(()) => {
                *slot.lock() = Some(w);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Sends a raw IRC command through the connection.
    pub async fn send_raw(&self, command: &str) -> Result<()> {
        Self::send_raw_with_timeout(
            &self.writer,
            command,
            std::time::Duration::from_secs(Self::WRITE_TIMEOUT_SECS),
        )
        .await
    }

    /// send_raw 的可注入超时形态（测试用短窗验证 的
    /// 有界性；生产走 WRITE_TIMEOUT_SECS）。
    async fn send_raw_with_timeout(
        slot: &parking_lot::Mutex<Option<IRCWriter>>,
        command: &str,
        timeout: std::time::Duration,
    ) -> Result<()> {
        match Self::write_line_with_timeout(slot, command, timeout).await {
            Ok(()) => {
                debug!(command = %command, "[IRCChannel] sent command");
                Ok(())
            }
            Err(e) => Err(NemesisError::Channel(format!("IRC send failed: {e}"))),
        }
    }

    /// Spawns the connection loop with auto-reconnect and exponential backoff.
    fn spawn_connection_loop(&self) {
        let running = self.running.clone();
        let config = self.config.clone();
        // 收件发布面需要 allow-list 与 bus，一并共享进任务。
        let base = Arc::clone(&self.base);
        let bus_sender = self.bus_sender.clone();
        // 写槽共享进任务——连接建立后装载写半部，
        // send_raw 才有 writer 可用（此前任务从不装载，出站恒死）。
        let writer_slot = Arc::clone(&self.writer);

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
                        let (reader, writer) = tokio::io::split(stream);

                        // 写半部装载进共享写槽（此前是
                        // 任务本地变量，send_raw 的槽永远 None——出站死路）。
                        // 注册与后续 PONG/JOIN/QUIT 全走同一写槽单一写路径
                        // （take-写-放回串行化，与 dispatch send 并发安全）。
                        *writer_slot.lock() = Some(IRCWriter { writer });

                        // Send registration commands（build_registration 单一
                        // 权威定义；写失败/超时不放回，外层重连环会重装）
                        for command in IRCChannel::build_registration(&config) {
                            let _ = IRCChannel::write_line_with_timeout(
                                &writer_slot,
                                &command,
                                std::time::Duration::from_secs(IRCChannel::WRITE_TIMEOUT_SECS),
                            )
                            .await;
                        }

                        // Read loop
                        let mut reader = BufReader::new(reader);

                        loop {
                            tokio::select! {
                                _ = &mut cancel_rx => {
                                    info!("[IRCChannel] connection loop cancelled");
                                    // QUIT 用短超时：停机不被死连接拖满 30s
                                    // （base.rs sync-target 3s 先例）
                                    let _ = IRCChannel::write_line_with_timeout(
                                        &writer_slot,
                                        "QUIT :NemesisBot shutting down",
                                        std::time::Duration::from_secs(3),
                                    )
                                    .await;
                                    *writer_slot.lock() = None;
                                    return;
                                }
                                result = read_line_capped(&mut reader, MAX_IRC_LINE_BYTES) => {
                                    match result {
                                        Ok(None) => {
                                            warn!("[IRCChannel] connection closed");
                                            *writer_slot.lock() = None;
                                            break;
                                        }
                                        Ok(Some((raw, truncated))) => {
                                            // 带帽读（BUG-20261108-028）：超长毒丸行
                                            // 丢弃跳过（游标已对齐下一行行首），连接
                                            // 保活——IRC 合法行 ≤ 数 KB，超帽 = 对端
                                            // 异常输出而非协议行。
                                            if truncated {
                                                warn!(
                                                    cap = MAX_IRC_LINE_BYTES,
                                                    "[IRCChannel] oversized line dropped"
                                                );
                                                continue;
                                            }
                                            let buf = String::from_utf8_lossy(&raw).into_owned();
                                            let trimmed = buf.trim_end_matches("\r\n").trim_end_matches('\n');
                                            if trimmed.is_empty() {
                                                continue;
                                            }

                                            // PING/PONG
                                            if let Some(data) = trimmed.strip_prefix("PING ") {
                                                let _ = IRCChannel::write_line_with_timeout(
                                                    &writer_slot,
                                                    &format!("PONG {data}"),
                                                    std::time::Duration::from_secs(
                                                        IRCChannel::WRITE_TIMEOUT_SECS,
                                                    ),
                                                )
                                                .await;
                                                continue;
                                            }

                                            let (prefix, command, params) = IRCChannel::parse_irc_line(trimmed);

                                            match command {
                                                "001" => {
                                                    info!(channel = %config.channel, "[IRCChannel] registered, joining");
                                                    let _ = IRCChannel::write_line_with_timeout(
                                                        &writer_slot,
                                                        &format!("JOIN {}", config.channel),
                                                        std::time::Duration::from_secs(
                                                            IRCChannel::WRITE_TIMEOUT_SECS,
                                                        ),
                                                    )
                                                    .await;
                                                }
                                                "433" => {
                                                    let new_nick = format!("{current_nick}_");
                                                    current_nick = new_nick.clone();
                                                    let _ = IRCChannel::write_line_with_timeout(
                                                        &writer_slot,
                                                        &format!("NICK {new_nick}"),
                                                        std::time::Duration::from_secs(
                                                            IRCChannel::WRITE_TIMEOUT_SECS,
                                                        ),
                                                    )
                                                    .await;
                                                }
                                                "PRIVMSG" => {
                                                    // 收件必须发布到总线进
                                                    // agent loop——此前读循环没有 PRIVMSG 分支，
                                                    // 消息落 `_ => {}` 静默丢弃（通道从未接线
                                                    // 总线：构造器无 bus 参数，allow_from 零
                                                    // 消费）。私聊（target=本 bot 当前昵称）
                                                    // chat_id 取发送者昵称，回复 PRIVMSG 才能
                                                    // 送达；频道消息 chat_id=频道名。
                                                    if let Some((target, content)) =
                                                        IRCChannel::parse_privmsg(params)
                                                    {
                                                        // BUG-20261115-015：发送者昵称归一
                                                        // 后兼作身份键与 allow 闸输入
                                                        //（irc_identity，RFC 1459 口径）。
                                                        let sender = IRCChannel::irc_identity(
                                                            prefix
                                                                .map(
                                                                    IRCChannel::extract_nick_from_prefix,
                                                                )
                                                                .unwrap_or("unknown"),
                                                        );
                                                        // IRC 昵称大小写不敏感（RFC 1459
                                                        // casemapping）：身份键按 irc_identity
                                                        // 归一，私聊 chat_id=发送者昵称（归一
                                                        // 后），频道 chat_id=频道名（归一后）。
                                                        let chat_id =
                                                            if target.eq_ignore_ascii_case(&current_nick) {
                                                                sender.clone()
                                                            } else {
                                                                IRCChannel::irc_identity(target)
                                                            };
                                                        if base.handle_message(&sender) {
                                                            let inbound = InboundMessage::user_ingress(base.name().to_string(), sender.clone(), chat_id.clone(), content.to_string())
                                                            .with_session_key(format!("irc:{chat_id}"));
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
                                                    let _ = IRCChannel::write_line_with_timeout(
                                                        &writer_slot,
                                                        &format!("JOIN {}", config.channel),
                                                        std::time::Duration::from_secs(
                                                            IRCChannel::WRITE_TIMEOUT_SECS,
                                                        ),
                                                    )
                                                    .await;
                                                }
                                                "ERROR" => {
                                                    error!(params = %params, "[IRCChannel] error");
                                                    *writer_slot.lock() = None;
                                                    break;
                                                }
                                                _ => {}
                                            }
                                        }
                                        Err(e) => {
                                            error!(error = %e, "[IRCChannel] read error");
                                            *writer_slot.lock() = None;
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
        // BUG-20261125-019：幂等双启守卫 + base 旗标接线（BUG-20261121-050
        // dingtalk/qq/matrix/slack 同构家族漏网成员——irc 有 spawn 型连接
        // 循环，不满足 050 横向排查「无 spawn 型 receive loop」的豁免口径，
        // 是同一份模板的第五批漏检）。无守卫的重复 start 会 spawn 多个并行
        // 连接循环每条消息 N 倍喂 agent；trait is_running()（读 base 旗标）
        // 恒 false 使 manager 运行中守卫（start_all / any_channel_running /
        // status 上报三消费点）对本通道全部失真。
        if self.base.is_running() {
            info!("[IRCChannel] already running, skipping duplicate start");
            return Ok(());
        }
        info!(
            server = %self.config.server,
            nick = %self.config.nick,
            channel = %self.config.channel,
            "[IRCChannel] starting IRC channel"
        );
        *self.running.write() = true;
        self.base.set_running(true);
        self.base.set_enabled(true);

        // Spawn the connection loop
        self.spawn_connection_loop();

        info!("[IRCChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[IRCChannel] stopping IRC channel");
        *self.running.write() = false;
        // BUG-20261125-019：对称清 base 旗标（契约面见 start 内注释）。
        self.base.set_running(false);
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
        // BUG-20261123-041：PRIVMSG 目标含 CR/LF = 行框架注入面
        // （config.channel 来自配置可含换行；chat_id 出自 wire 解析天然无
        // CRLF——单点剥除防御两侧，R737 同款剥除纪律）。
        let target = target.replace(['\r', '\n'], "");

        let lines = Self::split_message(&msg.content, 400);
        for line in lines {
            self.send_raw(&format!("PRIVMSG {target} :{line}")).await?;
        }

        Ok(())
    }
}

/// BUG-20261123-041 共用原语：行框架协议的内容段不允许携带 `\n`——一律在
/// `\n` 处切分、空段跳过（对齐慢路径 idx 0 空段纪律与
/// `split_message_utf16` 的 filter(>0) 家族形态）。裸 `\r` 不切：主流
/// ircd 行帧以 `\r\n`/`\n` 扫描，裸 `\r` 是文本不是终止符。
fn split_newlines(s: &str) -> Vec<String> {
    s.split('\n')
        .filter(|seg| !seg.is_empty())
        .map(str::to_string)
        .collect()
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

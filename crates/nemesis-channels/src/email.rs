//! Email channel (IMAP polling + SMTP sending, pure stdlib).
//!
//! Uses IMAP for polling inbound emails and SMTP for sending outbound emails.
//! Implements raw TCP connections with optional TLS. The IMAP implementation
//! uses a simple line-based protocol for SEARCH, FETCH, and STORE commands.
//! SMTP uses a simple connection with MAIL FROM / RCPT TO / DATA flow.

#![allow(dead_code)] // channel API client — full schema mirrored from Go, parts unused
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

use nemesis_utils::line_cap::read_line_capped;

// （审计修复）：SMTP/IMAP IO 超时。此前 connect/read/write 全部
// 无超时——黑洞服务器（不回 SYN 或收包不应答）会让调用永久挂起；而
// dispatch_loop 是单循环串行 await，一条挂死 = 全通道出站永久停摆。
// 超时后 io::Error(TimedOut) 沿既有 map_err 通道变成诚实错误返回。
const CONNECT_TIMEOUT_SECS: u64 = 15;
const IO_TIMEOUT_SECS: u64 = 30;

/// SMTP 单行字节帽（BUG-20261108-028）：裸 read_line 把整行无界积累进
/// 内存才返回——被替换/恶意的邮件服务器一行无限输出打爆宿主内存。
/// RFC 5321 状态行 ≤512 字节，64KB 远超任何合法行。
const MAX_SMTP_LINE_BYTES: usize = 64 * 1024;

/// IMAP 单行字节帽（BUG-20261108-028）：本实现的 FETCH literal 正文按行
/// 流经 read_line（send_command_multi 按行收集到 tagged 完成行为止），
/// 合法邮件正文行可达数十 KB（未折行的 base64/HTML），8MB 远超之。
/// 超帽截断续收（游标对齐下一行行首，连接保活）——病理邮件正文降级为
/// 截断版 + warn，对端失控输出不再无界入内存。
const MAX_IMAP_LINE_BYTES: usize = 8 * 1024 * 1024;

/// 包一层超时：超时映射 `io::ErrorKind:TimedOut`，调用方既有 `.map_err`
/// 照常拿到带上下文的错误（错误面文案 = `{what} timed out after Ns`）。
async fn io_timeout<T>(
    what: &str,
    dur: std::time::Duration,
    fut: impl std::future::Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    match tokio::time::timeout(dur, fut).await {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("{what} timed out after {}s", dur.as_secs()),
        )),
    }
}

/// 带帽读一行 SMTP 协议行（BUG-20261108-028）。超帽 = 对端异常输出，
/// 协议行失效 → Err（SMTP 状态行 ≤512 字节，帽内全部合法行）；
/// EOF → Ok("")（对齐裸 read_line Ok(0) 的既有空串语义——调用方状态行
/// 检查本就把空串当失败处理）。
async fn smtp_read_line<R>(reader: &mut R) -> std::io::Result<String>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    match read_line_capped(reader, MAX_SMTP_LINE_BYTES).await? {
        None => Ok(String::new()),
        Some((raw, truncated)) => {
            if truncated {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "SMTP response line exceeds cap",
                ));
            }
            Ok(String::from_utf8_lossy(&raw).into_owned())
        }
    }
}

/// Email channel configuration.
/// SEC-002（BUG-20261111-068）：IMAP/SMTP 认证材料（username/password 对）
/// 全脱敏——与 config 侧 MqttConfig 同原则。
#[derive(Clone)]
pub struct EmailConfig {
    /// IMAP host.
    pub imap_host: String,
    /// IMAP port (default: 993).
    pub imap_port: u16,
    /// IMAP username.
    pub imap_username: String,
    /// IMAP password.
    pub imap_password: String,
    /// Whether to use TLS for IMAP.
    pub imap_tls: bool,
    /// SMTP host.
    pub smtp_host: String,
    /// SMTP port (default: 587).
    pub smtp_port: u16,
    /// SMTP username (defaults to imap_username).
    pub smtp_username: Option<String>,
    /// SMTP password (defaults to imap_password).
    pub smtp_password: Option<String>,
    /// Whether to use TLS for SMTP.
    pub smtp_tls: bool,
    /// IMAP mailbox folder (default: "INBOX").
    pub folder: String,
    /// Poll interval in seconds (default: 30).
    pub poll_interval: u64,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

crate::debug_redact::impl_redacted_debug!(EmailConfig {
    plain { imap_host, imap_port, imap_tls, smtp_host, smtp_port, smtp_tls, folder, poll_interval, allow_from }
    secret { imap_username, imap_password }
    opt_secret { smtp_username, smtp_password }
});

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: 993,
            imap_username: String::new(),
            imap_password: String::new(),
            imap_tls: true,
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_username: None,
            smtp_password: None,
            smtp_tls: true,
            folder: "INBOX".to_string(),
            poll_interval: 30,
            allow_from: Vec::new(),
        }
    }
}

/// Stores metadata about an inbound email for reply routing.
#[derive(Debug, Clone)]
struct EmailMessageInfo {
    sender_email: String,
    subject: String,
}

/// Email channel using IMAP polling and SMTP sending.
pub struct EmailChannel {
    /// Arc 共享进轮询任务（allow-list 检查在 spawn 侧执行）。
    base: Arc<BaseChannel>,
    config: EmailConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    seen_uids: Arc<parking_lot::RwLock<HashMap<String, bool>>>,
    /// Maps chat_id -> EmailMessageInfo for reply routing.
    ///
    /// 必须 `Arc<DashMap>`——裸 `DashMap::clone()` 是逐
    /// shard 深拷贝，start() 交给 IMAP 轮询任务的副本与 send() 读的原表
    /// 分叉，回复线索写入恒不可见（Re: 主题静默落默认值）。
    sender_map: Arc<dashmap::DashMap<String, EmailMessageInfo>>,
    /// Message bus sender for publishing inbound emails。
    ///
    /// 与 external/qq/matrix 通道同构：收件必须发布到总线进 agent loop。
    /// 此前本通道构造器没有 bus 参数，轮询循环收到邮件只标已读即丢弃。
    bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    /// Cancellation sender for stopping the polling loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl EmailChannel {
    /// Creates a new `EmailChannel`.
    ///
    /// `bus_sender` is used to publish inbound emails read from IMAP
    /// (parity with external/qq/matrix channels' `(cfg, messageBus)` shape).
    pub fn new(
        config: EmailConfig,
        bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.imap_host.is_empty() || config.smtp_host.is_empty() {
            return Err(NemesisError::Channel(
                "email imap_host and smtp_host are required".to_string(),
            ));
        }
        if config.imap_username.is_empty() || config.imap_password.is_empty() {
            return Err(NemesisError::Channel(
                "email imap_username and imap_password are required".to_string(),
            ));
        }

        Ok(Self {
            base: Arc::new(BaseChannel::with_allow_list(
                "email",
                config.allow_from.clone(),
            )),
            config,
            running: Arc::new(parking_lot::RwLock::new(false)),
            seen_uids: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            sender_map: Arc::new(dashmap::DashMap::new()),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Returns the SMTP username (falls back to IMAP username).
    pub fn smtp_username(&self) -> &str {
        self.config
            .smtp_username
            .as_deref()
            .unwrap_or(&self.config.imap_username)
    }

    /// Returns the SMTP password (falls back to IMAP password).
    pub fn smtp_password(&self) -> &str {
        self.config
            .smtp_password
            .as_deref()
            .unwrap_or(&self.config.imap_password)
    }

    /// Extracts an email address from a From header.
    /// Examples: "John Doe <john@example.com>" -> "john@example.com"
    ///           "john@example.com" -> "john@example.com"
    pub fn extract_email_address(from: &str) -> &str {
        let from = from.trim();

        // Try angle brackets
        if let Some(start) = from.rfind('<')
            && let Some(end) = from.rfind('>')
            && end > start
        {
            return from[start + 1..end].trim();
        }

        // No angle brackets, check if it looks like an email
        if from.contains('@') { from } else { "" }
    }

    /// Parses IMAP SEARCH results into sequence numbers.
    pub fn parse_search_results(responses: &[String]) -> Vec<String> {
        let mut seq_nums = Vec::new();
        for resp in responses {
            if let Some(rest) = resp.strip_prefix("* SEARCH ") {
                for part in rest.split_whitespace() {
                    let p = part.trim();
                    // 纯数字闸（R737 协议注入面）：seq 原样拼进
                    // `STORE {seq} +FLAGS` / `FETCH {seq}` 命令流，IMAP 服务器
                    // 返回畸形 token 即可注入任意后续命令。合法 SEARCH 响应的
                    // token 恒为数字，非数字 token 丢弃。
                    if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) {
                        seq_nums.push(p.to_string());
                    }
                }
            }
        }
        seq_nums
    }

    /// Builds a reply subject.
    pub fn build_reply_subject(original_subject: &str) -> String {
        if original_subject.is_empty() {
            return "Re: NemesisBot Response".to_string();
        }
        let lower = original_subject.to_lowercase();
        if lower.starts_with("re:") {
            original_subject.to_string()
        } else {
            format!("Re: {original_subject}")
        }
    }

    /// seen_uids 去重图容量帽。键为机器键（每封收件的 Message-ID 一条目），
    /// 无淘汰则随收件量线性增长、永不回收 = 长驻进程内存慢漏。
    /// 家族纪律对齐（mastodon/nostr/signal 同款 10000→5000 修剪；bluesky 500
    /// 全清）：超帽修剪 5000 条（HashMap 任意序）——去重语义只需覆盖近期
    /// 重投递窗口，保护窗缩短为最近约 5000 封，可接受。
    const SEEN_UIDS_MAX_ENTRIES: usize = 10_000;
    const SEEN_UIDS_PRUNE_COUNT: usize = 5_000;

    /// 超帽修剪（两个插入点共用：mark_seen + start_poll_loop 内联插入）。
    fn prune_seen_uids(map: &mut HashMap<String, bool>) {
        if map.len() > Self::SEEN_UIDS_MAX_ENTRIES {
            let keys: Vec<String> = map
                .keys()
                .take(Self::SEEN_UIDS_PRUNE_COUNT)
                .cloned()
                .collect();
            for key in keys {
                map.remove(&key);
            }
        }
    }

    /// Marks a message ID as seen.
    pub fn mark_seen(&self, message_id: &str) {
        let mut map = self.seen_uids.write();
        map.insert(message_id.to_string(), true);
        Self::prune_seen_uids(&mut map);
    }

    /// Checks if a message ID has been seen.
    pub fn is_seen(&self, message_id: &str) -> bool {
        self.seen_uids.read().contains_key(message_id)
    }

    /// Builds IMAP LOGIN command arguments.
    pub fn build_imap_login(&self) -> String {
        let user = &self.config.imap_username;
        let pass = &self.config.imap_password;
        format!("LOGIN \"{user}\" \"{pass}\"")
    }

    /// Builds a full email message for SMTP.
    pub fn build_smtp_message(from: &str, to: &str, subject: &str, body: &str) -> String {
        let date = chrono::Utc::now().to_rfc2822();
        // 加固臂：RFC 5322 头部值是单行的——from/to/subject
        // 一旦混入 CR/LF 即构成头部注入（当前数据流经 IMAP 行解析不可达，
        // 但本函数是 pub 编码单一入口，纵深防御在编码点收敛；body 区由
        // SMTP dot-stuffing 负责，不受此影响）。CR/LF 直接剥除。
        let header_safe = |v: &str| {
            v.chars()
                .filter(|&c| c != '\r' && c != '\n')
                .collect::<String>()
        };
        format!(
            "From: {}\r\n\
             To: {}\r\n\
             Subject: {}\r\n\
             Date: {date}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=UTF-8\r\n\
             \r\n\
             {body}",
            header_safe(from),
            header_safe(to),
            header_safe(subject)
        )
    }

    /// Parses email headers from IMAP FETCH response lines.
    /// Returns (from, subject, message_id).
    pub fn parse_email_headers(responses: &[String]) -> (String, String, String) {
        let mut from = String::new();
        let mut subject = String::new();
        let mut message_id = String::new();
        let mut in_literal = false;
        let mut header_data = String::new();

        for line in responses {
            // Detect literal string marker {N}
            if line.contains('{')
                && line.contains('}')
                && let Some(start) = line.find('{')
                && let Some(end) = line.find('}')
                && end > start
            {
                in_literal = true;
                continue;
            }

            if in_literal {
                header_data.push_str(line);
                header_data.push_str("\r\n");
                continue;
            }

            let lower = line.to_lowercase();
            if lower.starts_with("from:") {
                from = line.trim_start_matches("From:").trim().to_string();
            } else if lower.starts_with("subject:") {
                subject = line.trim_start_matches("Subject:").trim().to_string();
            } else if lower.starts_with("message-id:") {
                message_id = line
                    .trim_start_matches("Message-ID:")
                    .trim()
                    .trim_matches(|c| c == '<' || c == '>' || c == ' ')
                    .to_string();
            }
        }

        // Also parse from the header literal data if present
        for hdr_line in header_data.split("\r\n") {
            let lower = hdr_line.to_lowercase();
            if lower.starts_with("from:") && from.is_empty() {
                from = hdr_line.trim_start_matches("From:").trim().to_string();
            } else if lower.starts_with("subject:") && subject.is_empty() {
                subject = hdr_line.trim_start_matches("Subject:").trim().to_string();
            } else if lower.starts_with("message-id:") && message_id.is_empty() {
                message_id = hdr_line
                    .trim_start_matches("Message-ID:")
                    .trim()
                    .trim_matches(|c| c == '<' || c == '>' || c == ' ')
                    .to_string();
            }
        }

        (from, subject, message_id)
    }

    /// Parses email body from IMAP FETCH BODY[TEXT] response lines.
    pub fn parse_email_body(responses: &[String]) -> String {
        if responses.is_empty() {
            return String::new();
        }

        let mut body_parts = Vec::new();
        for line in responses {
            if line.starts_with("* ") && line.contains("FETCH") {
                continue;
            }
            if line == ")" {
                continue;
            }
            body_parts.push(line.clone());
        }

        body_parts.join("\n").trim().to_string()
    }

    // -----------------------------------------------------------------------
    // SMTP protocol
    // -----------------------------------------------------------------------

    /// Connects to the SMTP server and sends an email.
    pub async fn smtp_send(&self, to: &str, subject: &str, body: &str) -> Result<()> {
        let addr = format!("{}:{}", self.config.smtp_host, self.config.smtp_port);

        let stream = io_timeout(
            "SMTP connect",
            std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
            TcpStream::connect(&addr),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP connect failed: {e}")))?;

        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Read greeting（带帽读，BUG-20261108-028）
        let buf = io_timeout(
            "SMTP read greeting",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            smtp_read_line(&mut reader),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP read greeting failed: {e}")))?;

        if !buf.starts_with("220") {
            return Err(NemesisError::Channel(format!(
                "SMTP unexpected greeting: {buf}"
            )));
        }

        // EHLO
        self.smtp_command(&mut writer, &mut reader, "EHLO nemesisbot.local")
            .await?;

        // For STARTTLS on port 587 (non-TLS mode would upgrade here)
        // For simplicity, we support direct TLS via separate connection

        // AUTH LOGIN
        self.smtp_command(&mut writer, &mut reader, "AUTH LOGIN")
            .await?;

        // Send username (base64)
        let user_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            self.smtp_username().as_bytes(),
        );
        self.smtp_command(&mut writer, &mut reader, &user_b64)
            .await?;

        // Send password (base64)
        let pass_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            self.smtp_password().as_bytes(),
        );
        self.smtp_command(&mut writer, &mut reader, &pass_b64)
            .await?;

        // MAIL FROM / RCPT TO（envelope 命令注入面，R737）：CR/LF 剥除——
        // envelope 值混入 CR/LF 即构成命令注入（可在 DATA 前引入额外
        // MAIL FROM/RCPT TO）；对齐 build_smtp_message 的 header_safe 既有
        // 纵深形态（头部已收敛，envelope 同面同收敛）。
        self.smtp_command(
            &mut writer,
            &mut reader,
            &format!("MAIL FROM:<{}>", smtp_envelope_safe(self.smtp_username())),
        )
        .await?;

        // RCPT TO
        self.smtp_command(
            &mut writer,
            &mut reader,
            &format!("RCPT TO:<{}>", smtp_envelope_safe(to)),
        )
        .await?;

        // DATA
        self.smtp_command(&mut writer, &mut reader, "DATA").await?;

        // Send message body
        let message = Self::build_smtp_message(self.smtp_username(), to, subject, body);
        // Escape dot-starting lines
        let message = message.replace("\r\n.", "\r\n..");
        io_timeout(
            "SMTP write body",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            writer.write_all(message.as_bytes()),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP write body failed: {e}")))?;
        io_timeout(
            "SMTP write terminator",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            writer.write_all(b"\r\n.\r\n"),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP write terminator failed: {e}")))?;

        // Read response（带帽读，BUG-20261108-028）
        let buf = io_timeout(
            "SMTP read DATA response",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            smtp_read_line(&mut reader),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP read DATA response failed: {e}")))?;

        if !buf.starts_with("250") {
            return Err(NemesisError::Channel(format!("SMTP DATA rejected: {buf}")));
        }

        // QUIT
        let _ = self.smtp_command(&mut writer, &mut reader, "QUIT").await;

        debug!(to = %to, "[EmailChannel] SMTP email sent successfully");
        Ok(())
    }

    /// Sends an SMTP command and reads the response.
    async fn smtp_command<W, R>(&self, writer: &mut W, reader: &mut R, command: &str) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
        R: tokio::io::AsyncBufRead + Unpin,
    {
        io_timeout(
            "SMTP write command",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            writer.write_all(format!("{command}\r\n").as_bytes()),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP write failed: {e}")))?;

        let mut buf = io_timeout(
            "SMTP read",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            smtp_read_line(reader),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("SMTP read failed: {e}")))?;

        // Check for multi-line responses (EHLO)
        while buf.len() >= 4 && buf.as_bytes()[3] == b'-' {
            buf = io_timeout(
                "SMTP read multi-line",
                std::time::Duration::from_secs(IO_TIMEOUT_SECS),
                smtp_read_line(reader),
            )
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP read multi-line failed: {e}")))?;
        }

        // Check response code
        let code = buf.get(..3).unwrap_or("");
        if code != "220" && code != "235" && code != "250" && code != "334" && code != "354" {
            return Err(NemesisError::Channel(format!(
                "SMTP command '{command}' failed: {buf}"
            )));
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // IMAP protocol
    // -----------------------------------------------------------------------

    /// Connects to the IMAP server, logs in, and selects the mailbox.
    pub async fn imap_connect(&self) -> Result<ImapConnection> {
        let addr = format!("{}:{}", self.config.imap_host, self.config.imap_port);

        let stream = io_timeout(
            "IMAP connect",
            std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
            TcpStream::connect(&addr),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("IMAP connect failed: {e}")))?;

        let (reader, writer) = tokio::io::split(stream);
        let mut conn = ImapConnection {
            reader: BufReader::new(reader),
            writer,
            tag_counter: 0,
        };

        // Read greeting
        let greeting = conn
            .read_line()
            .await
            .map_err(|e| NemesisError::Channel(format!("IMAP read greeting failed: {e}")))?;

        if !greeting.starts_with("* OK") {
            return Err(NemesisError::Channel(format!(
                "IMAP unexpected greeting: {greeting}"
            )));
        }

        // Login（IMAP quoted 注入面，R737：用户名/密码含 `"`/`\`/CRLF 会
        // 破坏 quoted string 边界——imap_quoted 转义 + 剥除）
        let login_cmd = format!(
            "LOGIN {} {}",
            imap_quoted(&self.config.imap_username),
            imap_quoted(&self.config.imap_password)
        );
        conn.send_command(&login_cmd).await?;

        // Select mailbox
        conn.send_command(&format!("SELECT {}", imap_quoted(&self.config.folder)))
            .await?;

        debug!(
            host = %self.config.imap_host,
            folder = %self.config.folder,
            "[EmailChannel] IMAP connected and authenticated"
        );

        Ok(conn)
    }

    /// Spawns the IMAP polling loop.
    fn start_poll_loop(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(tx);

        let running = self.running.clone();
        let config = self.config.clone();
        let seen_uids = self.seen_uids.clone();
        let sender_map = self.sender_map.clone();
        // 收件发布面需要 allow-list 与 bus，一并共享进任务。
        let base = Arc::clone(&self.base);
        let bus_sender = self.bus_sender.clone();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(config.poll_interval));
            // 默认 Burst 在挂起恢复后把错过的 tick 逐个补发
            // ——IMAP 服务器被密集轮询。Delay 从当前时刻重排。
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut rx = rx;

            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("[EmailChannel] IMAP poll loop shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        if !*running.read() {
                            break;
                        }
                        // Attempt to poll
                        if let Ok(mut conn) = imap_connect_static(&config).await {
                            // SEARCH UNSEEN
                            match conn.send_command_multi("SEARCH UNSEEN").await {
                                Ok(responses) => {
                                    let seq_nums = parse_search_results_static(&responses);
                                    for seq in seq_nums {
                                        // FETCH envelope
                                        let fetch_cmd = format!(
                                            "{} (ENVELOPE BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)])",
                                            seq
                                        );
                                        if let Ok(headers) = conn.send_command_multi(&format!("FETCH {}", fetch_cmd)).await {
                                            let (from, subject, message_id) =
                                                parse_email_headers_static(&headers);

                                            // Check seen
                                            {
                                                let mut map = seen_uids.write();
                                                if !message_id.is_empty() && map.contains_key(&message_id) {
                                                    continue;
                                                }
                                                if !message_id.is_empty() {
                                                    map.insert(message_id.clone(), true);
                                                    Self::prune_seen_uids(&mut map);
                                                }
                                            }

                                            // FETCH body（失败必须
                                            // 告警——此前.ok() 吞错，正文丢失无任何
                                            // 痕迹；取不到就不发布，避免造出空内容
                                            // 幻影会话。已入 seen_uids，下轮不会重处理）
                                            let body_cmd = format!("{} (BODY[TEXT])", seq);
                                            let body = match conn
                                                .send_command_multi(&format!("FETCH {}", body_cmd))
                                                .await
                                            {
                                                Ok(r) => parse_email_body_static(&r),
                                                Err(e) => {
                                                    warn!(error = %e, seq = %seq,
                                                        "[EmailChannel] failed to fetch email body");
                                                    continue;
                                                }
                                            };

                                            // Extract email address
                                            let sender_email =
                                                extract_email_address_static(&from);
                                            let chat_id = if sender_email.is_empty() {
                                                "email:unknown".to_string()
                                            } else {
                                                sender_email.clone()
                                            };

                                            // Store for replies
                                            if !sender_email.is_empty() {
                                                let reply_subject =
                                                    build_reply_subject_static(&subject);
                                                sender_map.insert(
                                                    chat_id.clone(),
                                                    EmailMessageInfo {
                                                        sender_email: sender_email.clone(),
                                                        subject: reply_subject,
                                                    },
                                                );
                                            }

                                            // Mark as seen
                                            let _ = conn
                                                .send_command(&format!(
                                                    "STORE {} +FLAGS (\\Seen)",
                                                    seq
                                                ))
                                                .await;

                                            // 收件必须发布到总线进
                                            // agent loop——此前整条收件路径到「标已读 +
                                            // debug 日志」为止，正文取了即丢（_body 未
                                            // 消费），bus 从未接线（构造器无 bus 参数），
                                            // allow_from 白名单也零消费：邮件进黑洞，
                                            // agent 永远看不到。无发件人地址的邮件不发布
                                            // （回复无处投递，与 sender_map 的写入闸一致）。
                                            if !sender_email.is_empty() {
                                                if base.handle_message(&chat_id) {
                                                    let mut metadata = HashMap::new();
                                                    metadata.insert(
                                                        "subject".to_string(),
                                                        subject.clone(),
                                                    );
                                                    let inbound = InboundMessage {
                                                        channel: base.name().to_string(),
                                                        sender_id: chat_id.clone(),
                                                        chat_id: chat_id.clone(),
                                                        content: body,
                                                        media: Vec::new(),
                                                        session_key: chat_id.clone(),
                                                        correlation_id: String::new(),
                                                        metadata,
                                                        voice_playback: None,
                                                    };
                                                    if let Err(e) = bus_sender.send(inbound) {
                                                        warn!(error = %e,
                                                            "[EmailChannel] failed to publish inbound message");
                                                    }
                                                } else {
                                                    warn!(chat_id = %chat_id,
                                                        "[EmailChannel] message blocked by allow-list");
                                                }
                                            }

                                            debug!(
                                                from = %from,
                                                subject = %subject,
                                                chat_id = %chat_id,
                                                "[EmailChannel] Received email"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "[EmailChannel] IMAP SEARCH failed");
                                }
                            }
                            let _ = conn.send_command("LOGOUT").await;
                        } else {
                            // 连接/认证失败不得静默——此前
                            // `if let Ok` 吞掉全部失败，IMAP 凭据错/网络断时
                            // 通道表现为「永远没有新邮件」，零日志可查。
                            warn!(
                                host = %config.imap_host,
                                "[EmailChannel] IMAP connect/auth failed; will retry next poll"
                            );
                        }
                    }
                }
            }
        });
    }
}

/// IMAP connection state.
pub struct ImapConnection {
    reader: BufReader<tokio::io::ReadHalf<tokio::net::TcpStream>>,
    writer: tokio::io::WriteHalf<tokio::net::TcpStream>,
    tag_counter: u32,
}

impl ImapConnection {
    /// Reads a single line from the IMAP connection.
    ///
    /// 这里是全部 IMAP 读的咽喉（greeting / poll / send_command* 都走它），
    /// 包一层超时即覆盖全部读路径。带帽读（BUG-20261108-028）：裸
    /// read_line 把整行无界积累进内存才返回——被替换/恶意的邮件服务器
    /// 一行无限输出打爆宿主内存；超帽截断续收（FETCH literal 正文按行
    /// 流经本咽喉，8MB 帽内全部合法正文行不受影响）。EOF：裸 read_line
    /// 返回 Ok("")，send_command* 收集循环把空行当 untagged 响应无限快转
    /// push（服务器断连 → Vec 无界增长 + 死循环）——改映射
    /// UnexpectedEof 让收集循环诚实失败走下轮重连。
    pub async fn read_line(&mut self) -> std::io::Result<String> {
        let (raw, truncated) = io_timeout(
            "IMAP read",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            read_line_capped(&mut self.reader, MAX_IMAP_LINE_BYTES),
        )
        .await?
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed")
        })?;
        if truncated {
            warn!(
                cap = MAX_IMAP_LINE_BYTES,
                "[EmailChannel] IMAP oversized line truncated"
            );
        }
        let mut line = String::from_utf8_lossy(&raw).into_owned();
        // Strip CRLF
        if line.ends_with("\r\n") {
            line.truncate(line.len() - 2);
        } else if line.ends_with('\n') {
            line.truncate(line.len() - 1);
        }
        Ok(line)
    }

    /// Generates the next tag.
    fn next_tag(&mut self) -> String {
        self.tag_counter += 1;
        format!("NB{:03}", self.tag_counter)
    }

    /// Sends a tagged IMAP command and reads until the tagged response.
    pub async fn send_command(&mut self, command: &str) -> Result<()> {
        let tag = self.next_tag();
        let cmd = format!("{tag} {command}\r\n");
        io_timeout(
            "IMAP write",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            self.writer.write_all(cmd.as_bytes()),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("IMAP write failed: {e}")))?;

        loop {
            let line = self
                .read_line()
                .await
                .map_err(|e| NemesisError::Channel(format!("IMAP read failed: {e}")))?;

            if line.starts_with(&format!("{tag} ")) {
                if line.contains("OK") {
                    return Ok(());
                }
                return Err(NemesisError::Channel(format!("IMAP error: {line}")));
            }
        }
    }

    /// Sends a tagged IMAP command and collects all untagged responses.
    pub async fn send_command_multi(&mut self, command: &str) -> Result<Vec<String>> {
        let tag = self.next_tag();
        let cmd = format!("{tag} {command}\r\n");
        io_timeout(
            "IMAP write",
            std::time::Duration::from_secs(IO_TIMEOUT_SECS),
            self.writer.write_all(cmd.as_bytes()),
        )
        .await
        .map_err(|e| NemesisError::Channel(format!("IMAP write failed: {e}")))?;

        let mut responses = Vec::new();
        loop {
            let line = self
                .read_line()
                .await
                .map_err(|e| NemesisError::Channel(format!("IMAP read failed: {e}")))?;

            if line.starts_with(&format!("{tag} ")) {
                if line.contains("OK") {
                    return Ok(responses);
                }
                return Err(NemesisError::Channel(format!("IMAP error: {line}")));
            }
            responses.push(line);
        }
    }
}

// Static helper functions for use in the spawned poll loop
async fn imap_connect_static(config: &EmailConfig) -> Result<ImapConnection> {
    let addr = format!("{}:{}", config.imap_host, config.imap_port);
    let stream = io_timeout(
        "IMAP connect",
        std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
        TcpStream::connect(&addr),
    )
    .await
    .map_err(|e| NemesisError::Channel(format!("IMAP connect failed: {e}")))?;

    let (reader, writer) = tokio::io::split(stream);
    let mut conn = ImapConnection {
        reader: BufReader::new(reader),
        writer,
        tag_counter: 0,
    };

    let greeting = conn
        .read_line()
        .await
        .map_err(|e| NemesisError::Channel(format!("IMAP read greeting failed: {e}")))?;

    if !greeting.starts_with("* OK") {
        return Err(NemesisError::Channel(format!(
            "IMAP unexpected greeting: {greeting}"
        )));
    }

    conn.send_command(&format!(
        "LOGIN {} {}",
        imap_quoted(&config.imap_username),
        imap_quoted(&config.imap_password)
    ))
    .await?;

    conn.send_command(&format!("SELECT {}", imap_quoted(&config.folder)))
        .await?;

    Ok(conn)
}

/// IMAP quoted string 编码（R737 协议注入面）：`"`/`\` 用 `\` 转义，
/// CR/LF 直接剥除（quoted 内不允许换行——配置值含换行即非法，
/// 剥除对齐 `build_smtp_message` 的 header_safe 既有形态）。
fn imap_quoted(s: &str) -> String {
    let escaped: String = s
        .chars()
        .filter(|&c| c != '\r' && c != '\n')
        .map(|c| match c {
            '"' | '\\' => format!("\\{c}"),
            _ => c.to_string(),
        })
        .collect();
    format!("\"{escaped}\"")
}

/// SMTP envelope 命令值安全化（R737 协议注入面）：CR/LF 直接剥除——
/// envelope 值混入 CR/LF 即构成命令注入（DATA 前引入额外命令行）；
/// 对齐 `build_smtp_message` 的 header_safe 既有形态。
fn smtp_envelope_safe(v: &str) -> String {
    v.chars().filter(|&c| c != '\r' && c != '\n').collect()
}

fn parse_search_results_static(responses: &[String]) -> Vec<String> {
    EmailChannel::parse_search_results(responses)
}

fn parse_email_headers_static(responses: &[String]) -> (String, String, String) {
    EmailChannel::parse_email_headers(responses)
}

fn parse_email_body_static(responses: &[String]) -> String {
    EmailChannel::parse_email_body(responses)
}

fn extract_email_address_static(from: &str) -> String {
    EmailChannel::extract_email_address(from).to_string()
}

fn build_reply_subject_static(subject: &str) -> String {
    EmailChannel::build_reply_subject(subject)
}

#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!(
            imap_host = %self.config.imap_host,
            smtp_host = %self.config.smtp_host,
            poll_interval = self.config.poll_interval,
            "[EmailChannel] starting Email channel"
        );
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Start IMAP polling loop
        self.start_poll_loop();

        info!("[EmailChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[EmailChannel] stopping Email channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Signal the polling loop to stop
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        self.sender_map.clear();
        info!("[EmailChannel] channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "email channel not running".to_string(),
            ));
        }

        if msg.chat_id.is_empty() {
            return Err(NemesisError::Channel(
                "no recipient email address in chat_id".to_string(),
            ));
        }

        self.base.record_sent();

        // Look up subject for reply threading
        let subject = self
            .sender_map
            .get(&msg.chat_id)
            .map(|info| info.subject.clone())
            .unwrap_or_else(|| "Re: NemesisBot Response".to_string());

        debug!(
            to = %msg.chat_id,
            subject = %subject,
            "[EmailChannel] sending message"
        );

        self.smtp_send(&msg.chat_id, &subject, &msg.content).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

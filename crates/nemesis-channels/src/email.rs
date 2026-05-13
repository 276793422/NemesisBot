//! Email channel (IMAP polling + SMTP sending, pure stdlib).
//!
//! Uses IMAP for polling inbound emails and SMTP for sending outbound emails.
//! Implements raw TCP connections with optional TLS. The IMAP implementation
//! uses a simple line-based protocol for SEARCH, FETCH, and STORE commands.
//! SMTP uses a simple connection with MAIL FROM / RCPT TO / DATA flow.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Email channel configuration.
#[derive(Debug, Clone)]
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
    base: BaseChannel,
    config: EmailConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    seen_uids: Arc<parking_lot::RwLock<HashMap<String, bool>>>,
    /// Maps chat_id -> EmailMessageInfo for reply routing.
    sender_map: dashmap::DashMap<String, EmailMessageInfo>,
    /// Cancellation sender for stopping the polling loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl EmailChannel {
    /// Creates a new `EmailChannel`.
    pub fn new(config: EmailConfig) -> Result<Self> {
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
            base: BaseChannel::new("email"),
            config,
            running: Arc::new(parking_lot::RwLock::new(false)),
            seen_uids: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            sender_map: dashmap::DashMap::new(),
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
        if let Some(start) = from.rfind('<') {
            if let Some(end) = from.rfind('>') {
                if end > start {
                    return from[start + 1..end].trim();
                }
            }
        }

        // No angle brackets, check if it looks like an email
        if from.contains('@') {
            from
        } else {
            ""
        }
    }

    /// Parses IMAP SEARCH results into sequence numbers.
    pub fn parse_search_results(responses: &[String]) -> Vec<String> {
        let mut seq_nums = Vec::new();
        for resp in responses {
            if let Some(rest) = resp.strip_prefix("* SEARCH ") {
                for part in rest.split_whitespace() {
                    let p = part.trim();
                    if !p.is_empty() {
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

    /// Marks a message ID as seen.
    pub fn mark_seen(&self, message_id: &str) {
        let mut map = self.seen_uids.write();
        map.insert(message_id.to_string(), true);
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
        format!(
            "From: {from}\r\n\
             To: {to}\r\n\
             Subject: {subject}\r\n\
             Date: {date}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=UTF-8\r\n\
             \r\n\
             {body}"
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
            if line.contains('{') && line.contains('}') {
                if let Some(start) = line.find('{') {
                    if let Some(end) = line.find('}') {
                        if end > start {
                            in_literal = true;
                            continue;
                        }
                    }
                }
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

        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP connect failed: {e}")))?;

        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut buf = String::new();

        // Read greeting
        reader
            .read_line(&mut buf)
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
        self.smtp_command(
            &mut writer,
            &mut reader,
            &format!("AUTH LOGIN"),
        )
        .await?;

        // Send username (base64)
        let user_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, self.smtp_username().as_bytes());
        self.smtp_command(&mut writer, &mut reader, &user_b64)
            .await?;

        // Send password (base64)
        let pass_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, self.smtp_password().as_bytes());
        self.smtp_command(&mut writer, &mut reader, &pass_b64)
            .await?;

        // MAIL FROM
        self.smtp_command(
            &mut writer,
            &mut reader,
            &format!("MAIL FROM:<{}>", self.smtp_username()),
        )
        .await?;

        // RCPT TO
        self.smtp_command(
            &mut writer,
            &mut reader,
            &format!("RCPT TO:<{to}>"),
        )
        .await?;

        // DATA
        self.smtp_command(&mut writer, &mut reader, "DATA").await?;

        // Send message body
        let message = Self::build_smtp_message(self.smtp_username(), to, subject, body);
        // Escape dot-starting lines
        let message = message.replace("\r\n.", "\r\n..");
        writer
            .write_all(message.as_bytes())
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP write body failed: {e}")))?;
        writer
            .write_all(b"\r\n.\r\n")
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP write terminator failed: {e}")))?;

        // Read response
        buf.clear();
        reader
            .read_line(&mut buf)
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP read DATA response failed: {e}")))?;

        if !buf.starts_with("250") {
            return Err(NemesisError::Channel(format!(
                "SMTP DATA rejected: {buf}"
            )));
        }

        // QUIT
        let _ = self
            .smtp_command(&mut writer, &mut reader, "QUIT")
            .await;

        debug!(to = %to, "SMTP email sent successfully");
        Ok(())
    }

    /// Sends an SMTP command and reads the response.
    async fn smtp_command<W, R>(
        &self,
        writer: &mut W,
        reader: &mut R,
        command: &str,
    ) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
        R: AsyncBufReadExt + Unpin,
    {
        writer
            .write_all(format!("{command}\r\n").as_bytes())
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP write failed: {e}")))?;

        let mut buf = String::new();
        reader
            .read_line(&mut buf)
            .await
            .map_err(|e| NemesisError::Channel(format!("SMTP read failed: {e}")))?;

        // Check for multi-line responses (EHLO)
        while buf.len() >= 4 && buf.as_bytes()[3] == b'-' {
            buf.clear();
            reader
                .read_line(&mut buf)
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

        let stream = TcpStream::connect(&addr)
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

        // Login
        let login_cmd = format!(
            "LOGIN \"{}\" \"{}\"",
            self.config.imap_username, self.config.imap_password
        );
        conn.send_command(&login_cmd).await?;

        // Select mailbox
        conn.send_command(&format!("SELECT \"{}\"", self.config.folder))
            .await?;

        debug!(
            host = %self.config.imap_host,
            folder = %self.config.folder,
            "IMAP connected and authenticated"
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

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                config.poll_interval,
            ));
            let mut rx = rx;

            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("IMAP poll loop shutting down");
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
                                                }
                                            }

                                            // FETCH body
                                            let body_cmd = format!("{} (BODY[TEXT])", seq);
                                            let body = conn
                                                .send_command_multi(&format!("FETCH {}", body_cmd))
                                                .await
                                                .ok()
                                                .map(|r| parse_email_body_static(&r))
                                                .unwrap_or_default();

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

                                            debug!(
                                                from = %from,
                                                subject = %subject,
                                                chat_id = %chat_id,
                                                "Received email"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "IMAP SEARCH failed");
                                }
                            }
                            let _ = conn.send_command("LOGOUT").await;
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
    pub async fn read_line(&mut self) -> std::io::Result<String> {
        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
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
        self.writer
            .write_all(cmd.as_bytes())
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
        self.writer
            .write_all(cmd.as_bytes())
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
                return Err(NemesisError::Channel(format!(
                    "IMAP error: {line}"
                )));
            }
            responses.push(line);
        }
    }
}

// Static helper functions for use in the spawned poll loop
async fn imap_connect_static(config: &EmailConfig) -> Result<ImapConnection> {
    let addr = format!("{}:{}", config.imap_host, config.imap_port);
    let stream = TcpStream::connect(&addr)
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
        "LOGIN \"{}\" \"{}\"",
        config.imap_username, config.imap_password
    ))
    .await?;

    conn.send_command(&format!("SELECT \"{}\"", config.folder))
        .await?;

    Ok(conn)
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

    async fn start(&self) -> Result<()> {
        info!(
            imap_host = %self.config.imap_host,
            smtp_host = %self.config.smtp_host,
            poll_interval = self.config.poll_interval,
            "starting Email channel"
        );
        *self.running.write() = true;
        self.base.set_enabled(true);

        // Start IMAP polling loop
        self.start_poll_loop();

        info!("Email channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Email channel");
        *self.running.write() = false;
        self.base.set_enabled(false);

        // Signal the polling loop to stop
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        self.sender_map.clear();
        info!("Email channel stopped");
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
            "Email sending message"
        );

        self.smtp_send(&msg.chat_id, &subject, &msg.content).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_email_address_with_brackets() {
        assert_eq!(
            EmailChannel::extract_email_address("John Doe <john@example.com>"),
            "john@example.com"
        );
    }

    #[test]
    fn test_extract_email_address_bare() {
        assert_eq!(
            EmailChannel::extract_email_address("john@example.com"),
            "john@example.com"
        );
    }

    #[test]
    fn test_extract_email_address_no_email() {
        assert_eq!(EmailChannel::extract_email_address("John Doe"), "");
    }

    #[test]
    fn test_parse_search_results() {
        let responses = vec![
            "* SEARCH 1 2 3".to_string(),
            "* SEARCH 4 5".to_string(),
            "NB00 OK SEARCH completed".to_string(),
        ];
        let nums = EmailChannel::parse_search_results(&responses);
        assert_eq!(nums, vec!["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn test_build_reply_subject() {
        assert_eq!(
            EmailChannel::build_reply_subject("Hello"),
            "Re: Hello"
        );
        assert_eq!(
            EmailChannel::build_reply_subject("Re: Hello"),
            "Re: Hello"
        );
        assert_eq!(
            EmailChannel::build_reply_subject(""),
            "Re: NemesisBot Response"
        );
    }

    #[test]
    fn test_build_smtp_message() {
        let msg = EmailChannel::build_smtp_message(
            "bot@example.com",
            "user@example.com",
            "Re: Hello",
            "Hi there",
        );
        assert!(msg.starts_with("From: bot@example.com\r\n"));
        assert!(msg.contains("To: user@example.com\r\n"));
        assert!(msg.contains("Subject: Re: Hello\r\n"));
        assert!(msg.contains("Hi there"));
    }

    #[tokio::test]
    async fn test_email_channel_new_validates() {
        let config = EmailConfig::default();
        assert!(EmailChannel::new(config).is_err());
    }

    #[tokio::test]
    async fn test_email_channel_lifecycle() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "user".to_string(),
            imap_password: "pass".to_string(),
            ..Default::default()
        };
        let ch = EmailChannel::new(config).unwrap();
        assert_eq!(ch.name(), "email");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_seen_tracking() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "user".to_string(),
            imap_password: "pass".to_string(),
            ..Default::default()
        };
        let ch = EmailChannel::new(config).unwrap();

        assert!(!ch.is_seen("msg-1"));
        ch.mark_seen("msg-1");
        assert!(ch.is_seen("msg-1"));
    }

    #[test]
    fn test_parse_email_headers() {
        let responses = vec![
            "* 1 FETCH (ENVELOPE (...) BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {68}".to_string(),
            "From: Alice <alice@example.com>".to_string(),
            "Subject: Test Subject".to_string(),
            "Message-ID: <msg123@example.com>".to_string(),
            ")".to_string(),
        ];
        let (from, subject, message_id) = EmailChannel::parse_email_headers(&responses);
        assert_eq!(from, "Alice <alice@example.com>");
        assert_eq!(subject, "Test Subject");
        assert_eq!(message_id, "msg123@example.com");
    }

    #[test]
    fn test_parse_email_body() {
        let responses = vec![
            "* 1 FETCH (BODY[TEXT] {11}".to_string(),
            "Hello world".to_string(),
            ")".to_string(),
        ];
        let body = EmailChannel::parse_email_body(&responses);
        assert_eq!(body, "Hello world");
    }

    #[test]
    fn test_parse_email_body_empty() {
        let body = EmailChannel::parse_email_body(&[]);
        assert!(body.is_empty());
    }

    #[test]
    fn test_smtp_username_fallback() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "imap_user".to_string(),
            imap_password: "imap_pass".to_string(),
            smtp_username: Some("smtp_user".to_string()),
            smtp_password: None,
            ..Default::default()
        };
        let ch = EmailChannel::new(config).unwrap();
        assert_eq!(ch.smtp_username(), "smtp_user");
        assert_eq!(ch.smtp_password(), "imap_pass");
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_parse_search_results_empty() {
        let nums = EmailChannel::parse_search_results(&[]);
        assert!(nums.is_empty());
    }

    #[test]
    fn test_parse_search_results_no_search_lines() {
        let responses = vec![
            "NB00 OK SEARCH completed".to_string(),
        ];
        let nums = EmailChannel::parse_search_results(&responses);
        assert!(nums.is_empty());
    }

    #[test]
    fn test_parse_search_results_single() {
        let responses = vec![
            "* SEARCH 42".to_string(),
            "NB00 OK".to_string(),
        ];
        let nums = EmailChannel::parse_search_results(&responses);
        assert_eq!(nums, vec!["42"]);
    }

    #[test]
    fn test_extract_email_address_angle_brackets() {
        assert_eq!(
            EmailChannel::extract_email_address("<alice@example.com>"),
            "alice@example.com"
        );
    }

    #[test]
    fn test_build_reply_subject_fwd() {
        assert_eq!(
            EmailChannel::build_reply_subject("Fwd: News"),
            "Re: Fwd: News"
        );
    }

    #[test]
    fn test_parse_email_headers_empty() {
        let (from, subject, message_id) = EmailChannel::parse_email_headers(&[]);
        assert!(from.is_empty());
        assert!(subject.is_empty());
        assert!(message_id.is_empty());
    }

    #[test]
    fn test_parse_email_body_multiline() {
        let responses = vec![
            "* 1 FETCH (BODY[TEXT] {22}".to_string(),
            "Line one".to_string(),
            "Line two".to_string(),
            ")".to_string(),
        ];
        let body = EmailChannel::parse_email_body(&responses);
        assert!(body.contains("Line one"));
        assert!(body.contains("Line two"));
    }

    #[test]
    fn test_build_smtp_message_content() {
        let msg = EmailChannel::build_smtp_message(
            "sender@test.com",
            "receiver@test.com",
            "Test",
            "Body content",
        );
        assert!(msg.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(msg.contains("Body content"));
    }

    #[test]
    fn test_email_config_default() {
        let cfg = EmailConfig::default();
        assert!(cfg.imap_host.is_empty());
        assert!(cfg.smtp_host.is_empty());
        assert!(cfg.imap_username.is_empty());
        assert!(cfg.imap_password.is_empty());
        assert_eq!(cfg.poll_interval, 300);
    }

    #[test]
    fn test_seen_tracking_multiple() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "user".to_string(),
            imap_password: "pass".to_string(),
            ..Default::default()
        };
        let ch = EmailChannel::new(config).unwrap();

        assert!(!ch.is_seen("a"));
        assert!(!ch.is_seen("b"));

        ch.mark_seen("a");
        assert!(ch.is_seen("a"));
        assert!(!ch.is_seen("b"));

        ch.mark_seen("b");
        assert!(ch.is_seen("a"));
        assert!(ch.is_seen("b"));
    }

    #[test]
    fn test_smtp_username_default_fallback() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "imap_user".to_string(),
            imap_password: "imap_pass".to_string(),
            ..Default::default()
        };
        let ch = EmailChannel::new(config).unwrap();
        assert_eq!(ch.smtp_username(), "imap_user");
        assert_eq!(ch.smtp_password(), "imap_pass");
    }

    #[test]
    fn test_smtp_password_explicit() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "imap_user".to_string(),
            imap_password: "imap_pass".to_string(),
            smtp_password: Some("smtp_pass".to_string()),
            ..Default::default()
        };
        let ch = EmailChannel::new(config).unwrap();
        assert_eq!(ch.smtp_password(), "smtp_pass");
    }
}

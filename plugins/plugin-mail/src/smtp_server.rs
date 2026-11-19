//! SMTP 服务端——收信入口（两级判定）+ 拒中继 + 超限 552。
//!
//! RCPT 两级收信判定（多地址注册制 v1 地基）：
//! 1. 域级：RCPT 域 ∈ `local_domains`（大小写不敏感）——不在 = 550（这同时
//!    就是拒中继语义：本机域之外一律拒，open relay 面为零）；
//! 2. 地址级：RCPT 完整地址已注册（`addresses.json` 在册）——未注册 = 550
//!    （注册制管「收不收」，命名自由管「叫什么」）。
//!
//! 会话纪律：命令行 CRLF 终止（2KB 限长防膨胀）；DATA 以单独 `.` 行终止 +
//! 反点填充（服务端收到的是**已填充**载荷——`.` 行首剥一个点还原原文）；
//! 载荷超 `max_message_bytes` → 552（收满即断，防内存膨胀）；信封值经
//! `smtp_envelope_safe` 拍平（CR/LF 注入面在解析点防御）。
//!
//! STARTTLS（RFC 3207，opportunistic）：`smtp.tls` 配了证书 → EHLO 广告
//! `250 STARTTLS`；客户端发起 → 220 → 服务端握手 → **会话状态全清**
//! （MAIL/RCPT 丢弃 + 要求新 EHLO——RFC 3207 §4.2 防明文阶段状态注入）；
//! 未配证书不广告（明文互操作不破——MX 惯例 opportunistic）。
//!
//! 反垃圾最小集（产品层，不靠部署环境兜底）：
//! - `max_messages_per_connection`：单连接投递数上限（默认 20，0=不限），
//!   超限 452 + 断开——公网端点防刷信灌库；
//! - `max_rcpt_failures`：单连接 RCPT 拒绝累计上限（默认 10，0=不限），
//!   超限 421 断开——地址枚举/暴力面收口；
//! - `idle_timeout_secs`：会话空闲超时（默认 300s，0=不限）——慢速攻击
//!   挂连接面收口。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use nemesis_mail_protocol::{extract_email_address, smtp_envelope_safe};

use crate::store::MailStore;

/// SMTP 段配置（`config.mail.json` `smtp` 节的 resolved 形态）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct SmtpSettings {
    /// 本面开关（server_start 按各自 enabled 起停）。
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    pub local_domains: Vec<String>,
    pub max_message_bytes: u64,
    /// STARTTLS 证书对（enabled + 路径；装配失败 = 拒绝启动）。
    pub tls: crate::TlsServerSettings,
    /// 单连接投递数上限（0=不限）。默认 20。
    pub max_messages_per_connection: u32,
    /// 单连接 RCPT 拒绝累计上限（0=不限）。默认 10。
    pub max_rcpt_failures: u32,
    /// 会话空闲超时秒（0=不限）。默认 300。
    pub idle_timeout_secs: u64,
}

impl Default for SmtpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1".to_string(),
            port: 2525,
            local_domains: Vec::new(),
            max_message_bytes: 10 * 1024 * 1024,
            tls: crate::TlsServerSettings::default(),
            max_messages_per_connection: 20,
            max_rcpt_failures: 10,
            idle_timeout_secs: 300,
        }
    }
}

/// 会话上下文（store + 宿主回调桥）。
pub struct SmtpCtx {
    pub store: Arc<MailStore>,
    pub host: Arc<crate::MailHost>,
}

/// accept 循环（listener 由调用方 bind 好——端口 0 场景须把真实端口带回去）。
/// shutdown 关闭 → 优雅退出（listener drop）。acceptor = STARTTLS 证书闸
/// （None = 明文面，不广告 STARTTLS——opportunistic 不破互操作）。
pub async fn run(
    listener: TcpListener,
    ctx: Arc<SmtpCtx>,
    settings: Arc<SmtpSettings>,
    acceptor: Option<Arc<TlsAcceptor>>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                crate::log(crate::MAIL_LOG_INFO, "smtp", "服务端停机（shutdown 信号）");
                break;
            }
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let ctx = ctx.clone();
                let settings = settings.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(handle_session(
                    stream,
                    ctx,
                    settings,
                    acceptor,
                    peer.to_string(),
                ));
            }
        }
    }
}

/// 会话 IO 形态（STARTTLS 中途升级——命令循环跨形态续跑，RFC 3207）。
enum Conn {
    Plain(
        BufReader<tokio::net::tcp::OwnedReadHalf>,
        tokio::net::tcp::OwnedWriteHalf,
    ),
    Tls(
        BufReader<tokio::io::ReadHalf<tokio_rustls::server::TlsStream<TcpStream>>>,
        tokio::io::WriteHalf<tokio_rustls::server::TlsStream<TcpStream>>,
    ),
}

impl Conn {
    /// 读一行（CRLF 终止；None = 流结束/IO 错误/超限/idle 超时）。限长 2KB。
    async fn read_line(&mut self, idle: Option<Duration>) -> Option<String> {
        match self {
            Conn::Plain(r, _) => read_line_generic(r, idle).await,
            Conn::Tls(r, _) => read_line_generic(r, idle).await,
        }
    }

    /// 单行应答（写失败 = 会话断开，静默——对端感知即可，accept 循环不炸）。
    async fn send(&mut self, line: &str) {
        let bytes = format!("{line}\r\n");
        let _ = match self {
            Conn::Plain(_, w) => w.write_all(bytes.as_bytes()).await,
            Conn::Tls(_, w) => w.write_all(bytes.as_bytes()).await,
        };
    }

    /// 还原裸 TCP（STARTTLS 升级前置——BufReader 缓冲丢弃是协议保证：RFC 3207
    /// 客户端必须等 220 后才发 TLS ClientHello，缓冲区无残留命令）。
    fn take_tcp(self) -> Option<TcpStream> {
        match self {
            Conn::Plain(r, w) => r.into_inner().reunite(w).ok(),
            Conn::Tls(..) => None,
        }
    }
}

/// 单会话处理（命令循环；IO 错误/idle 超时/闸断开 = 会话结束，不炸 accept 循环）。
async fn handle_session(
    stream: TcpStream,
    ctx: Arc<SmtpCtx>,
    settings: Arc<SmtpSettings>,
    acceptor: Option<Arc<TlsAcceptor>>,
    _peer: String,
) {
    let idle = crate::idle_or_max(settings.idle_timeout_secs);
    let (reader, writer) = stream.into_split();
    let mut conn = Conn::Plain(BufReader::new(reader), writer);
    conn.send("220 mailbot SMTP ready").await;

    let mut mail_from: Option<String> = None;
    let mut rcpts: Vec<String> = Vec::new();
    // 反垃圾计数（产品层闸，不靠部署环境兜底）。
    let mut msgs_delivered: u32 = 0;
    let mut rcpt_failures: u32 = 0;
    let mut in_tls = false;

    loop {
        let Some(line) = conn.read_line(idle).await else {
            return; // 对端断开 / IO 错误 / idle 超时
        };
        let cmd = line.trim_end();
        let upper = cmd.to_uppercase();

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            conn.send("250-mailbot Hello").await;
            // STARTTLS 广告（opportunistic）：配了证书且本会话尚未升级才广告。
            if acceptor.is_some() && !in_tls {
                conn.send("250-STARTTLS").await;
            }
            conn.send("250 OK").await;
        } else if upper == "STARTTLS" {
            match (acceptor.as_ref(), in_tls) {
                (Some(a), false) => {
                    conn.send("220 Ready to start TLS").await;
                    // in_tls == false ⇒ conn 必为 Plain（类型不变量）——
                    // take_tcp 必有值，None 分支仅返回即可达。
                    let Some(tcp) = conn.take_tcp() else { return };
                    match a.accept(tcp).await {
                        Ok(tls) => {
                            let (r, w) = tokio::io::split(tls);
                            conn = Conn::Tls(BufReader::new(r), w);
                            in_tls = true;
                            // RFC 3207 §4.2：TLS 层建立 = 会话状态全清——
                            // 明文阶段的 MAIL/RCPT 信封必须丢弃（防状态注入）。
                            mail_from = None;
                            rcpts.clear();
                            crate::log(
                                crate::MAIL_LOG_INFO,
                                "smtp",
                                "STARTTLS 握手完成（会话状态已重置）",
                            );
                        }
                        Err(e) => {
                            crate::log(
                                crate::MAIL_LOG_INFO,
                                "smtp",
                                &format!("STARTTLS 握手失败: {e}"),
                            );
                            return; // 握手失败断开——TLS 层错误不回协议层
                        }
                    }
                }
                (None, _) => conn.send("502 STARTTLS not available").await,
                (_, true) => conn.send("503 TLS already active").await,
            }
        } else if let Some(rest) = strip_ci(cmd, "MAIL FROM:") {
            let addr = extract_envelope_addr(Some(rest));
            mail_from = Some(addr);
            rcpts.clear();
            conn.send("250 OK").await;
        } else if let Some(rest) = strip_ci(cmd, "RCPT TO:") {
            let raw = extract_envelope_addr(Some(rest));
            match accept_rcpt(&raw, &ctx.store, &settings.local_domains) {
                Ok(()) => {
                    rcpts.push(raw);
                    conn.send("250 OK").await;
                }
                Err(reason) => {
                    crate::log(
                        crate::MAIL_LOG_INFO,
                        "smtp",
                        &format!("RCPT 拒收 {raw}: {reason}"),
                    );
                    // RCPT 失败闸：容忍 max 次拒绝（各 550 正常拒收语义），
                    // 第 max+1 次 → 421 断开（地址枚举/暴力面收口）。
                    rcpt_failures = rcpt_failures.saturating_add(1);
                    if settings.max_rcpt_failures != 0 && rcpt_failures > settings.max_rcpt_failures
                    {
                        crate::log(
                            crate::MAIL_LOG_INFO,
                            "smtp",
                            &format!("RCPT 失败数超限（{}）断开", settings.max_rcpt_failures),
                        );
                        conn.send(&format!(
                            "421 too many failures, closing connection (limit {})",
                            settings.max_rcpt_failures
                        ))
                        .await;
                        return;
                    }
                    conn.send(&format!("550 {reason}")).await;
                }
            }
        } else if upper == "DATA" {
            if mail_from.is_none() || rcpts.is_empty() {
                conn.send("503 need MAIL and RCPT first").await;
                continue;
            }
            // 投递数闸：到限的下一个 DATA → 452 + 断开（公网端点防刷信灌库）。
            if settings.max_messages_per_connection != 0
                && msgs_delivered >= settings.max_messages_per_connection
            {
                crate::log(
                    crate::MAIL_LOG_INFO,
                    "smtp",
                    &format!(
                        "单连接投递数超限（{}）断开",
                        settings.max_messages_per_connection
                    ),
                );
                conn.send("452 too many messages on this connection").await;
                return;
            }
            // RFC 5321：DATA 就绪必答 354（真实客户端等它才发载荷——漏发
            // 会让对端会话挂死，测试读行错位假绿也源于此）。
            conn.send("354 Start mail input, end with <CRLF>.<CRLF>")
                .await;
            match read_data(&mut conn, settings.max_message_bytes, idle).await {
                Ok(data) => match deliver_all(&ctx, mail_from.as_deref(), &rcpts, &data) {
                    Ok(first_id) => {
                        msgs_delivered = msgs_delivered.saturating_add(1);
                        conn.send(&format!("250 OK queued as {first_id}")).await;
                    }
                    Err(e) => {
                        crate::log(crate::MAIL_LOG_ERROR, "smtp", &format!("投递失败: {e}"));
                        conn.send(&format!("451 local error: {e}")).await;
                    }
                },
                Err(DataError::TooLarge) => {
                    // 收满即断载荷读取已终止——552 明确语义。
                    conn.send("552 message exceeds maximum size").await;
                }
                Err(DataError::Io) => return,
            }
        } else if upper == "RSET" {
            mail_from = None;
            rcpts.clear();
            conn.send("250 OK").await;
        } else if upper == "NOOP" {
            conn.send("250 OK").await;
        } else if upper.starts_with("VRFY") {
            conn.send("252 cannot verify, but will attempt delivery")
                .await;
        } else if upper == "QUIT" {
            conn.send("221 mailbot closing").await;
            return;
        } else {
            conn.send("500 unrecognized command").await;
        }
    }
}

/// 读一行通用体（BufReader 泛型——Plain/Tls 两形态同源）。
async fn read_line_generic<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    idle: Option<Duration>,
) -> Option<String> {
    let mut line = String::new();
    // read_line 读到 \n 为止；CRLF 会话中 \r 由下方 trim。None 形态区分：
    // Ok(0) = 流结束 → None；Err = IO 错误（含 idle 超时）→ None；超限 → None。
    let res = if let Some(t) = idle {
        tokio::time::timeout(t, reader.read_line(&mut line))
            .await
            .unwrap_or(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "idle timeout",
            )))
    } else {
        reader.read_line(&mut line).await
    };
    match res {
        Ok(0) => return None,
        Ok(_) if line.len() > 2048 => return None,
        Ok(_) => {}
        Err(_) => return None,
    }
    let mut s = line;
    while s.ends_with('\n') || s.ends_with('\r') {
        s.pop();
    }
    Some(s)
}

enum DataError {
    TooLarge,
    Io,
}

/// 读 DATA 载荷（`.` 单独行终止 + 反点填充还原；idle 超时与会话命令闸同源
/// ——载荷中途挂起不放行）。
async fn read_data(
    conn: &mut Conn,
    max_bytes: u64,
    idle: Option<Duration>,
) -> Result<String, DataError> {
    let mut out = String::new();
    loop {
        let Some(line) = conn.read_line(idle).await else {
            return Err(DataError::Io);
        };
        if line == "." {
            return Ok(out);
        }
        // 反点填充：行首 '.' 剥一个点（"..x" → ".x"；"." 已作终止符处理）。
        let unstuffed = line.strip_prefix('.').unwrap_or(&line);
        out.push_str(unstuffed);
        out.push_str("\r\n");
        if out.len() as u64 > max_bytes {
            return Err(DataError::TooLarge);
        }
    }
}

/// 信封地址提取（`<x@y>` 尖括号形态或裸地址；经 envelope_safe 拍平）。
fn extract_envelope_addr(rest: Option<&str>) -> String {
    let raw = rest.unwrap_or("");
    let trimmed = raw.trim();
    let addr = if let (Some(s), Some(e)) = (trimmed.find('<'), trimmed.rfind('>')) {
        &trimmed[s + 1..e]
    } else {
        trimmed
    };
    smtp_envelope_safe(extract_email_address(addr))
}

/// 大小写不敏感前缀剥离（RFC 5321 动词不分大小写；返回前缀后的剩余部分）。
fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// RCPT 两级判定：域 ∈ local_domains 且地址已注册。
fn accept_rcpt(addr: &str, store: &MailStore, domains: &[String]) -> Result<(), String> {
    let Some((_, domain)) = addr.rsplit_once('@') else {
        return Err("address rejected: malformed".to_string());
    };
    let domain_lc = domain.to_lowercase();
    if !domains.iter().any(|d| d.to_lowercase() == domain_lc) {
        // 域不在本机域 = 拒中继（open relay 面为零）。
        return Err("relay access denied".to_string());
    }
    let registered = store
        .list_addresses()
        .map_err(|e| format!("local error: {e}"))?;
    if !registered.iter().any(|a| a.address == addr) {
        return Err("address rejected: not registered".to_string());
    }
    Ok(())
}

/// 投递到全部 RCPT（每个已注册收件人一个库）；返回首个 id。
fn deliver_all(
    ctx: &SmtpCtx,
    from: Option<&str>,
    rcpts: &[String],
    data: &str,
) -> Result<String, String> {
    let mut first_id = String::new();
    for rcpt in rcpts {
        let id = ctx.store.deliver(rcpt, data)?;
        if first_id.is_empty() {
            first_id = id.clone();
        }
        let subject = crate::store::extract_header(data, "Subject").unwrap_or_default();
        ctx.host.audit(
            "mail.deliver",
            &serde_json::json!({"id": id, "mailbox": rcpt, "from": from.unwrap_or("")}).to_string(),
        );
        ctx.host.notify_mail_in(
            &serde_json::json!({
                "id": id,
                "mailbox": rcpt,
                "from": from.unwrap_or(""),
                "to": rcpt,
                "subject": subject,
                "ts_ms": now_ms(),
            })
            .to_string(),
        );
    }
    if first_id.is_empty() {
        return Err("无有效收件人".to_string());
    }
    Ok(first_id)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

//! POP3 服务端——RFC 1939 最小集（USER/PASS/STAT/LIST/UIDL/RETR/TOP/DELE/
//! RSET/NOOP/QUIT）+ 按地址库映射。
//!
//! 库映射（多地址注册制）：
//! - `USER <注册地址>` → 本会话只看该地址的库；
//! - 不发 USER（免认证形态）→ 全部注册地址库并集；
//! - `USER <未注册地址>` → `-ERR unknown user`（注册闸在枚举面诚实拒绝）。
//!
//! UIDL 带地址前缀（`<地址>/<文件名>`）——并集模式下跨库文件名（同毫秒
//! 时钟回拨极端场景）不重名，客户端去重锚不串库。
//!
//! 会话快照：库列表在进入 TRANSACTION 态时定格（会话中途新到信不出现，
//! 下一会话可见——RFC 1939 语义）。DELE 只做会话级标记，QUIT 时真删
//! （落库）；RSET 清标记。
//!
//! RETR/TOP 出站载荷重新点填充（POP3 终止符与 SMTP 同构——载荷中行首 `.`
//! 必须双写，客户端剥还原）。
//!
//! 认证：`pop3.password` 非空 = PASS 必须精确匹配（空 PASS 拒）；为空 =
//! 开放（并集）。无 APOP。
//!
//! 安全闸（产品层 fail-closed，不靠部署环境兜底）：
//! - 非回环绑址 + 免认证 → [`admission`] 拒绝启动（并集对任何可达者可见，
//!   没有合法非回环场景，不可豁免）；
//! - 非回环绑址 + 明文认证（未启 TLS）→ 默认拒绝启动，`allow_plaintext_auth`
//!   显式逃生（Dovecot disable_plaintext_auth 同型——LAN 内网自担风险）；
//! - 回环绑址不受限（本机测试/调试合法）；
//! - POP3S：`tls.enabled` 整面 implicit TLS（握手失败断开，不回协议错误）；
//!   TLS 装配失败 = 启动失败（fail-closed）；
//! - 会话空闲超时 `idle_timeout_secs`（默认 600s；0=不限）——慢速攻击
//!   挂连接面收口。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use crate::store::MailStore;

/// POP3 段配置（resolved 形态；`password_ref` 由宿主解析后以 `password`
/// 注入——插件不见 vault）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Pop3Settings {
    /// 本面开关（server_start 按各自 enabled 起停）。
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    /// 认证用户名（信息面；闸只看 password）。
    pub username: String,
    /// 非空 = PASS 必须匹配；空 = 开放。
    pub password: String,
    /// POP3S（整面 implicit TLS）：enabled + 证书对；装配失败 = 拒绝启动。
    pub tls: crate::TlsServerSettings,
    /// 非回环 + 明文认证（无 TLS）逃生阀（默认 false = 拒绝启动）。
    /// Dovecot disable_plaintext_auth 同型——LAN 内网明文需显式自担。
    pub allow_plaintext_auth: bool,
    /// 会话空闲超时秒（单条读等待上限；0 = 不限）。默认 600。
    pub idle_timeout_secs: u64,
}

impl Default for Pop3Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1".to_string(),
            port: 1110,
            username: String::new(),
            password: String::new(),
            tls: crate::TlsServerSettings::default(),
            allow_plaintext_auth: false,
            idle_timeout_secs: 600,
        }
    }
}

pub struct Pop3Ctx {
    pub store: Arc<MailStore>,
    pub host: Arc<crate::MailHost>,
}

/// 启动准入闸（bind 前零副作用纯判定 + TLS 装配；失败 = 拒绝启动）。
///
/// 回环绑址全放行；非回环按三态：
/// 1. TLS 在 → 通过（POP3S 正道）；
/// 2. 免认证（password 空）→ **拒绝，不可豁免**（并集全库对任何可达者可见）；
/// 3. 明文密码 + 未显式 `allow_plaintext_auth` → **拒绝**（默认 fail-closed，
///    显式逃生 = LAN 内网自担）。
pub fn admission(cfg: &Pop3Settings) -> Result<Option<Arc<TlsAcceptor>>, String> {
    let tls = crate::build_face_tls(&cfg.tls, "POP3")?;
    if crate::is_loopback_bind(&cfg.bind) {
        return Ok(tls.map(crate::to_acceptor));
    }
    if cfg.password.is_empty() {
        return Err(format!(
            "POP3 拒绝启动：免认证形态禁止非回环绑址 {}（全部地址库对任何可达者可见）\
             ——配置 pop3.password，或把 bind 收回 127.0.0.1",
            cfg.bind
        ));
    }
    if tls.is_none() && !cfg.allow_plaintext_auth {
        return Err(format!(
            "POP3 拒绝启动：非回环绑址 {} + 明文认证（未启用 pop3.tls）——\
             公网部署请启用 POP3S（pop3.tls.enabled + 证书）；\
             内网明文需显式设 pop3.allow_plaintext_auth = true（自担风险）",
            cfg.bind
        ));
    }
    Ok(tls.map(crate::to_acceptor))
}

/// 会话内消息条目（TRANSACTION 快照定格）。
struct SnapMsg {
    id: String,
    path: PathBuf,
    size: u64,
    uidl: String,
    deleted: bool,
}

/// accept 循环（acceptor = POP3S；None = 明文——非回环明文形态已在 admission
/// 闸被拒，此处 None 只可能来自回环绑址）。
pub async fn run(
    listener: TcpListener,
    ctx: Arc<Pop3Ctx>,
    settings: Arc<Pop3Settings>,
    acceptor: Option<Arc<TlsAcceptor>>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                crate::log(crate::MAIL_LOG_INFO, "pop3", "服务端停机（shutdown 信号）");
                break;
            }
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let ctx = ctx.clone();
                let settings = settings.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    match acceptor {
                        Some(a) => match a.accept(stream).await {
                            Ok(s) => handle_session(s, ctx, settings).await,
                            Err(e) => crate::log(
                                crate::MAIL_LOG_INFO,
                                "pop3",
                                &format!("TLS 握手失败（{peer}）：{e}"),
                            ),
                        },
                        None => handle_session(stream, ctx, settings).await,
                    }
                });
            }
        }
    }
}

async fn handle_session<S>(stream: S, ctx: Arc<Pop3Ctx>, settings: Arc<Pop3Settings>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let idle = crate::idle_or_max(settings.idle_timeout_secs);
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let _ = writer.write_all(b"+OK mailbot POP3 ready\r\n").await;

    // USER 解析结果：None = 未发 USER（免认证并集形态）；Some(addr) = 本库。
    let mut mailbox: Option<String> = None;
    // USER 指名未注册地址后的锁定位：fail-open 防线——指定了不存在的库就
    // 不允许静默回落并集视图（显式点名 ≠ 未点名，语义必须分开）。
    let mut user_rejected = false;
    let mut authed = settings.password.is_empty(); // 开放形态直接视为已认证
    let mut snap: Vec<SnapMsg> = Vec::new();

    loop {
        let Some(line) = read_line(&mut reader, idle).await else {
            return;
        };
        let trimmed = line.trim_end();
        let (cmd, arg) = match trimmed.split_once(' ') {
            Some((c, a)) => (c.to_uppercase(), a.trim().to_string()),
            None => (trimmed.to_uppercase(), String::new()),
        };

        let resp = match cmd.as_str() {
            "USER" => {
                // USER 任何形态都改变库视图语义——旧快照必须作废重建
                // （不清则换库后 STAT 复用上一视图：并集误报单库计数）。
                snap.clear();
                if arg.is_empty() {
                    // 空 USER = 并集形态入口。
                    mailbox = None;
                    user_rejected = false;
                    "+OK union mailbox mode\r\n".to_string()
                } else {
                    let registered = ctx.store.list_addresses().unwrap_or_default();
                    if registered.iter().any(|a| a.address == arg) {
                        mailbox = Some(arg);
                        user_rejected = false;
                        "+OK\r\n".to_string()
                    } else {
                        mailbox = None;
                        user_rejected = true;
                        "-ERR unknown user\r\n".to_string()
                    }
                }
            }
            "PASS" => {
                if authed {
                    // 开放形态重复 PASS / 无 USER 直接 PASS：宽容 +OK。
                    "+OK\r\n".to_string()
                } else if arg == settings.password {
                    authed = true;
                    "+OK authenticated\r\n".to_string()
                } else {
                    "-ERR authentication failed\r\n".to_string()
                }
            }
            "APOP" => "-ERR APOP not supported\r\n".to_string(),
            "STAT" | "LIST" | "UIDL" | "RETR" | "TOP" | "DELE" | "RSET" | "NOOP" | "QUIT" => {
                if !authed {
                    "-ERR not authenticated\r\n".to_string()
                } else if user_rejected {
                    "-ERR unknown user\r\n".to_string()
                } else if cmd == "QUIT" {
                    apply_deletes(&ctx, &snap);
                    let _ = writer.write_all(b"+OK mailbot signing off\r\n").await;
                    return;
                } else {
                    if snap.is_empty() {
                        snap = build_snapshot(&ctx.store, mailbox.as_deref());
                    }
                    transaction(&mut snap, &cmd, &arg).await
                }
            }
            _ => "-ERR unrecognized command\r\n".to_string(),
        };
        if writer.write_all(resp.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// TRANSACTION 态命令应答（STAT/LIST/UIDL/RETR/TOP/DELE/RSET/NOOP）。
async fn transaction(snap: &mut [SnapMsg], cmd: &str, arg: &str) -> String {
    match cmd {
        "STAT" => {
            let (n, bytes) = snap
                .iter()
                .filter(|m| !m.deleted)
                .fold((0u32, 0u64), |(n, b), m| (n + 1, b + m.size));
            format!("+OK {n} {bytes}\r\n")
        }
        "LIST" => {
            let msgs: Vec<&SnapMsg> = snap.iter().filter(|m| !m.deleted).collect();
            if !arg.is_empty() {
                return msg_by_num(&msgs, arg)
                    .map(|m| {
                        let num = msgs.iter().position(|x| x.id == m.id).unwrap_or(0) + 1;
                        format!("+OK {num} {}\r\n", m.size)
                    })
                    .unwrap_or_else(|| "-ERR no such message".to_string());
            }
            let total: u64 = msgs.iter().map(|m| m.size).sum();
            // RFC 1939 §5 BNF：LIST 无参首行 = "+OK" SP <count> SP <octets>（两整数，
            // 不带 "messages (… octets)" 人类可读后缀——STAT 同款严格形态）。
            let mut out = format!("+OK {} {total}\r\n", msgs.len());
            for (i, m) in msgs.iter().enumerate() {
                out.push_str(&format!("{} {}\r\n", i + 1, m.size));
            }
            out.push_str(".\r\n");
            out
        }
        "UIDL" => {
            let msgs: Vec<&SnapMsg> = snap.iter().filter(|m| !m.deleted).collect();
            if !arg.is_empty() {
                return msg_by_num(&msgs, arg)
                    .map(|m| {
                        let num = msgs.iter().position(|x| x.id == m.id).unwrap_or(0) + 1;
                        format!("+OK {num} {}\r\n", m.uidl)
                    })
                    .unwrap_or_else(|| "-ERR no such message".to_string());
            }
            let mut out = "+OK\r\n".to_string();
            for (i, m) in msgs.iter().enumerate() {
                out.push_str(&format!("{} {}\r\n", i + 1, m.uidl));
            }
            out.push_str(".\r\n");
            out
        }
        "RETR" => {
            let msgs: Vec<&SnapMsg> = snap.iter().filter(|m| !m.deleted).collect();
            let Some(m) = msg_by_num(&msgs, arg) else {
                return "-ERR no such message".to_string();
            };
            let Ok(eml) = std::fs::read_to_string(&m.path) else {
                return "-ERR message unreadable".to_string();
            };
            let mut out = format!("+OK {} octets\r\n", eml.len());
            out.push_str(&dot_stuff(&eml));
            out.push_str(".\r\n");
            out
        }
        "TOP" => {
            let mut parts = arg.split_whitespace();
            let (Some(num), Some(lines)) = (parts.next(), parts.next()) else {
                return "-ERR TOP requires two arguments".to_string();
            };
            let Ok(k) = lines.parse::<usize>() else {
                return "-ERR bad line count".to_string();
            };
            let msgs: Vec<&SnapMsg> = snap.iter().filter(|m| !m.deleted).collect();
            let Some(m) = msg_by_num(&msgs, num) else {
                return "-ERR no such message".to_string();
            };
            let Ok(eml) = std::fs::read_to_string(&m.path) else {
                return "-ERR message unreadable".to_string();
            };
            let (headers, body) = split_headers_body(&eml);
            let head_lines: Vec<&str> = body.lines().take(k).collect();
            let mut out = String::from("+OK\r\n");
            out.push_str(&headers);
            out.push_str("\r\n\r\n");
            out.push_str(&head_lines.join("\r\n"));
            out.push_str("\r\n.\r\n");
            out
        }
        "DELE" => {
            let msgs: Vec<&SnapMsg> = snap.iter().filter(|m| !m.deleted).collect();
            let Some(target) = msg_by_num(&msgs, arg) else {
                return "-ERR no such message".to_string();
            };
            // 会话级标记：按 uidl 找回完整快照条目置位（不可变借用先行结束
            // 再可变借用——msgs 持有 &snap 期间不能 iter_mut）。
            let uidl = target.uidl.clone();
            drop(msgs);
            if let Some(full) = snap.iter_mut().find(|x| x.uidl == uidl) {
                full.deleted = true;
            }
            "+OK message deleted\r\n".to_string()
        }
        "RSET" => {
            for m in snap.iter_mut() {
                m.deleted = false;
            }
            "+OK\r\n".to_string()
        }
        "NOOP" => "+OK\r\n".to_string(),
        _ => "-ERR unrecognized command\r\n".to_string(),
    }
}

/// QUIT 落库真删（DELE 标记的消息）。
fn apply_deletes(ctx: &Pop3Ctx, snap: &[SnapMsg]) {
    for m in snap.iter().filter(|m| m.deleted) {
        if let Err(e) = ctx.store.delete(&m.id) {
            crate::log(
                crate::MAIL_LOG_WARN,
                "pop3",
                &format!("QUIT 删除失败 {}: {e}", m.id),
            );
        } else {
            ctx.host.audit(
                "mail.delete",
                &serde_json::json!({"id": m.id, "via": "pop3"}).to_string(),
            );
        }
    }
}

/// 快照构建（并集 = 全部注册地址库；单库 = 指定地址）。
fn build_snapshot(store: &MailStore, mailbox: Option<&str>) -> Vec<SnapMsg> {
    let mut out = Vec::new();
    let boxes: Vec<String> = match mailbox {
        Some(addr) => vec![addr.to_string()],
        None => match store.list_addresses() {
            Ok(reg) => reg.into_iter().map(|a| a.address).collect(),
            Err(_) => Vec::new(),
        },
    };
    for b in boxes {
        let dir = store.root().join(&b);
        for (sub, _seen) in [("new", false), ("cur", true)] {
            let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("eml") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let id = name.trim_end_matches(".eml").to_string();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(SnapMsg {
                    uidl: format!("{b}/{name}"),
                    id,
                    path: path.clone(),
                    size,
                    deleted: false,
                });
            }
        }
    }
    // 文件名前缀 = 时间戳定宽 → 字典序升序 = 时间升序（稳定序号语义）。
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 1-based 消息号 → 快照条目。
fn msg_by_num<'a>(msgs: &[&'a SnapMsg], num: &str) -> Option<&'a SnapMsg> {
    let n: usize = num.parse().ok()?;
    if n == 0 {
        return None;
    }
    msgs.get(n - 1).copied()
}

/// POP3 出站点填充（行首 `.` 双写）。
fn dot_stuff(eml: &str) -> String {
    let mut out = String::with_capacity(eml.len() + 16);
    for line in eml.split_inclusive('\n') {
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\r\n");
        }
    }
    out
}

/// 头块 / 正文切分。
fn split_headers_body(eml: &str) -> (String, String) {
    match eml.find("\r\n\r\n") {
        Some(i) => (eml[..i].to_string(), eml[i + 4..].to_string()),
        None => match eml.find("\n\n") {
            Some(i) => (eml[..i].to_string(), eml[i + 2..].to_string()),
            None => (eml.to_string(), String::new()),
        },
    }
}

async fn read_line<S>(
    reader: &mut BufReader<tokio::io::ReadHalf<S>>,
    idle: Option<Duration>,
) -> Option<String>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut line = String::new();
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

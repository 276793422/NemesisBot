//! MailClient——出站发信（SMTP implicit TLS 465 + AUTH LOGIN）。
//!
//! 协议编码单一真相源：消息组装/载荷点填充走 `nemesis_mail_protocol`
//! （build_smtp_message / prepare_smtp_payload——与 email 通道同源）；TLS
//! 配置走同源 `tls_client_config`（显式 `ca_cert_path` 优先，fail-closed
//! ——缺省回落 webpki 公共根，公共根也装不上 = 发信诚实报错）。
//!
//! 继承协议层能力边界：只做 465 implicit TLS（587 STARTTLS 不支持）；
//! AUTH LOGIN（base64）；单收件人（`to` 一个地址——群发由调用方多次调用）。
//!
//! 发送完成后宿主侧经 `notify_mail_sent` 收结果回执（lib.rs 出口统一桥）。

use nemesis_mail_protocol::{build_smtp_message, extract_message_id, prepare_smtp_payload};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls;

/// client 段配置（resolved 形态；`password_ref` 由宿主解析后以 `password`
/// 注入——插件不见 vault）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ClientSettings {
    /// 本面开关（mail_send 拒绝未启用面）。
    pub enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub ca_cert_path: String,
    /// 缺省发件人（空 = 用 username）。
    pub from: String,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            smtp_host: String::new(),
            smtp_port: 465,
            username: String::new(),
            password: String::new(),
            ca_cert_path: String::new(),
            from: String::new(),
        }
    }
}

/// 发信。返回载荷 Message-ID（build_smtp_message 生成写入头——从构造产物
/// 提取，保证回执 id 与实际发出的载荷一致；服务器 250 应答不含 id）。
pub async fn send(
    st: &ClientSettings,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<String, String> {
    if !st.smtp_host.contains('.') && !st.smtp_host.contains(':') {
        // host 形态粗检（DNS 名或 IP：字面量）。
        if st.smtp_host.is_empty() {
            return Err("client.smtp_host 未配置".to_string());
        }
    }
    if st.username.is_empty() {
        return Err("client.username 未配置".to_string());
    }
    let from = if st.from.is_empty() {
        st.username.clone()
    } else {
        st.from.clone()
    };

    let config = build_tls_config(&st.ca_cert_path)?;
    let connector = TlsConnector::from(Arc::new(config));

    let server_name = rustls::pki_types::ServerName::try_from(st.smtp_host.clone())
        .map_err(|e| format!("TLS server name 非法（{}）：{e}", st.smtp_host))?;

    let tcp = TcpStream::connect((st.smtp_host.as_str(), st.smtp_port))
        .await
        .map_err(|e| format!("连接 {0}:{1} 失败：{e}", st.smtp_host, st.smtp_port))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| format!("TLS 握手失败（{}:{}）：{e}", st.smtp_host, st.smtp_port))?;

    let (reader, mut writer) = tokio::io::split(tls);
    let mut reader = BufReader::new(reader);

    // 1. greeting（220）
    read_response(&mut reader, 220).await?;
    // 2. EHLO（多行 250-... 直至 250 空格）
    ehlo(&mut reader, &mut writer).await?;
    // 3. AUTH LOGIN（password 配置了才走；空 = 无认证 relay 形态）
    if !st.password.is_empty() {
        auth_login(&mut reader, &mut writer, &st.username, &st.password).await?;
    }
    // 4. MAIL FROM / RCPT TO / DATA
    expect(
        &mut reader,
        &mut writer,
        &format!("MAIL FROM:<{from}>"),
        250,
    )
    .await?;
    expect(&mut reader, &mut writer, &format!("RCPT TO:<{to}>"), 250).await?;
    expect(&mut reader, &mut writer, "DATA", 354).await?;
    // 5. 载荷（编码单一真相源 + 点填充）+ 终止符
    let message = build_smtp_message(&from, to, subject, body);
    let payload = prepare_smtp_payload(&message);
    writer
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| format!("载荷发送失败：{e}"))?;
    writer
        .write_all(b"\r\n.\r\n")
        .await
        .map_err(|e| format!("载荷终止符发送失败：{e}"))?;
    read_response(&mut reader, 250).await?;
    // 6. QUIT（221；失败不影响已发出的事实）
    let _ = expect(&mut reader, &mut writer, "QUIT", 221).await;

    Ok(extract_message_id(&message))
}

fn build_tls_config(ca_cert_path: &str) -> Result<rustls::ClientConfig, String> {
    if ca_cert_path.is_empty() {
        // 缺省：webpki 公共根（fail-closed 的便利缺省；公共根装配不可能失败
        // ——内嵌常量）。
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = rustls::crypto::ring::default_provider();
        return Ok(rustls::ClientConfig::builder_with_provider(provider.into())
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS 协议版本装配失败：{e}"))?
            .with_root_certificates(roots)
            .with_no_client_auth());
    }
    let pem = std::fs::read(ca_cert_path)
        .map_err(|e| format!("ca_cert_path 读取失败（{ca_cert_path}）：{e}"))?;
    nemesis_mail_protocol::tls_client_config(&pem)
}

/// 读一行服务器响应（限长 8KB）。
async fn read_line(
    reader: &mut BufReader<
        tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    >,
) -> Result<String, String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| format!("响应读取失败：{e}"))?;
    if line.len() > 8192 {
        return Err("响应行超限".to_string());
    }
    Ok(line)
}

/// 读多行响应（`250-xxx` 续行直至 `250 xxx`），校验末行码。
async fn read_response(
    reader: &mut BufReader<
        tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    >,
    expect: u16,
) -> Result<String, String> {
    loop {
        let line = read_line(reader).await?;
        let trimmed = line.trim_end();
        if trimmed.len() < 3 {
            return Err(format!("响应畸形：{trimmed:?}"));
        }
        let code: u16 = trimmed
            .get(0..3)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("响应码非数字：{trimmed:?}"))?;
        let cont = trimmed.as_bytes().get(3) == Some(&b'-');
        if !cont {
            if code != expect {
                return Err(format!("响应码 {code}（期望 {expect}）：{trimmed}"));
            }
            return Ok(trimmed.to_string());
        }
    }
}

async fn write_line(
    writer: &mut tokio::io::WriteHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    line: &str,
) -> Result<(), String> {
    writer
        .write_all(format!("{line}\r\n").as_bytes())
        .await
        .map_err(|e| format!("命令发送失败：{e}"))
}

async fn ehlo(
    reader: &mut BufReader<
        tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    >,
    writer: &mut tokio::io::WriteHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
) -> Result<(), String> {
    write_line(writer, "EHLO mailbot").await?;
    read_response(reader, 250).await?;
    Ok(())
}

async fn auth_login(
    reader: &mut BufReader<
        tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    >,
    writer: &mut tokio::io::WriteHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    username: &str,
    password: &str,
) -> Result<(), String> {
    use base64::Engine;
    write_line(writer, "AUTH LOGIN").await?;
    read_response(reader, 334).await?;
    let user_b64 = base64::engine::general_purpose::STANDARD.encode(username.as_bytes());
    write_line(writer, &user_b64).await?;
    read_response(reader, 334).await?;
    let pass_b64 = base64::engine::general_purpose::STANDARD.encode(password.as_bytes());
    write_line(writer, &pass_b64).await?;
    read_response(reader, 235).await?;
    Ok(())
}

async fn expect(
    reader: &mut BufReader<
        tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    >,
    writer: &mut tokio::io::WriteHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    cmd: &str,
    code: u16,
) -> Result<String, String> {
    write_line(writer, cmd).await?;
    read_response(reader, code).await
}

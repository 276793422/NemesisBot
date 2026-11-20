//! 邮件协议层（单一真相源）——SMTP 出站消息编码 / RFC 5322 头解析 / TLS
//! 客户端配置。
//!
//! **纯函数零 tokio**（`tls_client_config` 含 rustls 依赖但只装配配置无 IO）。
//! 消费方两处，协议逻辑禁止在任一消费方内复制：
//! - `crates/nemesis-channels/src/email.rs`（email 通道，IMAP 轮询 + SMTP 发送）；
//! - `plugins/plugin-mail`（邮件基础设施插件 DLL，path 依赖同源、编译进产物，
//!   产物仍自包含）。
//!
//! 错误约定：本 crate 零 nemesis-* 依赖（插件 path 依赖的重量闸），错误以
//! `String` 返回（`tls_client_config`），消费方自行包装成本族错误类型
//! （email.rs → `NemesisError::Channel`）。
//!
//! 函数清单与 `email.rs` 内的注释/测试基线一一对应——搬移自 email.rs
//! （2026-11-18 邮件基础设施子系统 B1，纯搬移零行为变化，388 测试基线钉死）。

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

/// Generates a RFC 5322 Message-ID（`<millis.nanos@from域>`）。
///
/// 生成单一真相源：`build_smtp_message` 写头与调用方回执（plugin-mail
/// client）都必须经此函数——两处 id 一致靠「先构造后从消息里提取」达成
/// （[`extract_message_id`]），不是两次调用（两次调用值不同 = 回执造假）。
pub fn generate_message_id(from: &str) -> String {
    // 域名净化（BUG-20261120-039）：先剥显示名形态（"Name <a@b.com>" 直接
    // rsplit_once('@') 会取到 "com>" 畸形域），再剥 CR/LF（纵深防御对齐
    // `build_smtp_message` 的 header_safe 同族——Message-ID 是第四条 from
    // 衍生头输出，此前仅 From/To/Subject 三处加固，本行漏网即头断行注入
    // 面）。裸地址两步恒等（extract_email_address 原样返回；无 CR/LF 时
    // 剥除零变化）——既有裸地址测试零漂移。
    let addr = extract_email_address(from);
    let domain = addr
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or("mailbot.local");
    let domain: String = domain.chars().filter(|&c| c != '\r' && c != '\n').collect();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("<{millis}.{nanos}@{domain}>")
}

/// Extracts the Message-ID header value from a built SMTP message
///（`build_smtp_message` 产物形态已知——头区单行直查）。缺头 = 空串。
pub fn extract_message_id(message: &str) -> String {
    for line in message.split("\r\n") {
        if let Some(v) = line.strip_prefix("Message-ID: ") {
            return v.trim().to_string();
        }
    }
    String::new()
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
    // 非 ASCII Subject → RFC 2047 B-encoding（BUG-20261118-130：裸 8-bit
    // 头违反 RFC 5322 头 ASCII 域——未协商 SMTPUTF8 的会话严格 MTA 可
    // 拒收，收件端 MUA 按本地编码猜测乱码）。header_safe 先于编码：
    // 编码输入恒为拍平后的单行（注入面语义保持）。
    let subject = encode_rfc2047_subject(&header_safe(subject));
    // 非 ASCII 正文 → 显式 CTE: base64（RFC 2045 缺省 CTE=7bit 与实际
    // 8-bit UTF-8 不符面：严格 MTA 拒信或 8→7bit 转换损坏）；纯 ASCII
    // 正文零漂移（无 CTE 头，行为不变）。
    let (cte_line, body_out) = encode_body_mime(body);
    // Message-ID（RFC 5322 §3.6.4 SHOULD；QQ/163 等公共邮箱反垃圾评分项，
    // 缺头扣分甚至拒收）：内部生成写入头——零签名破坏（4 参签名不变，
    // email 通道白赚）；回执 id 一致性靠调用方从构造产物提取
    // （[`extract_message_id`]），不二次调用（两次调用值不同 = 回执造假）。
    let message_id = generate_message_id(from);
    format!(
        "From: {}\r\n\
         To: {}\r\n\
         Subject: {subject}\r\n\
         Date: {date}\r\n\
         Message-ID: {message_id}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=UTF-8\r\n\
         {cte_line}\
         \r\n\
         {body_out}",
        header_safe(from),
        header_safe(to),
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

        apply_header_field(
            line,
            &mut from,
            &mut subject,
            &mut message_id,
            FieldWriteMode::Overwrite,
        );
    }

    // Also parse from the header literal data if present
    for hdr_line in header_data.split("\r\n") {
        apply_header_field(
            hdr_line,
            &mut from,
            &mut subject,
            &mut message_id,
            FieldWriteMode::KeepExisting,
        );
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
        body_parts.push(line.clone());
    }

    // 信封终结符只剥末行（BUG-20261120-040）：literal 按声明字节精确消费
    // 后切行追加（email.rs `read_literal_lines`），正文含 ")" 独立行与
    // FETCH 响应信封 ")" 同形——anywhere 剥除会把正文行静默吞掉。终结符
    // 在响应形态中恒为末行（single UID FETCH 单 untagged 响应，literal 后
    // 独立成行），位置收紧后正文 ")" 保留、信封 ")" 照剥。
    if body_parts
        .last()
        .map(|l| l.as_str() == ")")
        .unwrap_or(false)
    {
        body_parts.pop();
    }

    body_parts.join("\n").trim().to_string()
}

/// SMTP envelope 命令值安全化（R737 协议注入面）：CR/LF 直接剥除——
/// envelope 值混入 CR/LF 即构成命令注入（DATA 前引入额外命令行）；
/// 对齐 `build_smtp_message` 的 header_safe 既有形态。
pub fn smtp_envelope_safe(v: &str) -> String {
    v.chars().filter(|&c| c != '\r' && c != '\n').collect()
}

/// RFC 2047 B-encoding（BUG-20261118-130，出站非 ASCII Subject）：RFC 5322
/// 头部值是 ASCII 域——裸 8-bit 头在未协商 SMTPUTF8 的会话可被严格 MTA
/// 拒收（554），收件端 MUA 按本地编码猜测乱码，反垃圾栈罚分。非 ASCII →
/// `=?UTF-8?B?...?=` encoded-word 序列：载荷按 48 base64 字符切块（4 对齐
/// ——每块独立可解，RFC 2047 §2 相邻 encoded-word 间的空白解码时丢弃），
/// 整个 encoded-word 60 ≤ RFC 2047 的 75 字符 SHOULD；相邻块以 CRLF+WSP
/// 折行衔接（RFC 5322 折行形态，行长有界不破 998 硬限）。纯 ASCII 原样
/// 返回（字节不变，既有测试与 prompt 面零漂移）。
pub fn encode_rfc2047_subject(v: &str) -> String {
    if v.is_ascii() {
        return v.to_string();
    }
    const CHUNK: usize = 48;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, v.as_bytes());
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < b64.len() {
        let end = (i + CHUNK).min(b64.len());
        parts.push(format!("=?UTF-8?B?{}?=", &b64[i..end]));
        i = end;
    }
    parts.join("\r\n ")
}

/// 正文 MIME CTE 面（BUG-20261118-130）：RFC 2045 §6.1 缺省 CTE=7bit——
/// 裸 8-bit UTF-8 正文未声明即发，严格 MTA 拒信或 qmail 类 8→7bit 转换
/// 损坏。非 ASCII 正文 → 显式 `Content-Transfer-Encoding: base64` 头 +
/// base64 载荷（76 字符折行 = RFC 2045 base64 行长硬限；base64 字母表无
/// '.'，与 dot-stuffing 正交）。纯 ASCII 正文原样（无 CTE 头，行为不变）。
/// 返回 (CTE 头行含自身 CRLF——空串=不加头, 编码后正文)。
pub fn encode_body_mime(body: &str) -> (String, String) {
    if body.is_ascii() {
        return (String::new(), body.to_string());
    }
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, body.as_bytes());
    let mut wrapped = String::with_capacity(b64.len() + b64.len() / 76 * 2 + 2);
    let mut i = 0usize;
    while i < b64.len() {
        let end = (i + 76).min(b64.len());
        if i > 0 {
            wrapped.push_str("\r\n");
        }
        wrapped.push_str(&b64[i..end]);
        i = end;
    }
    ("Content-Transfer-Encoding: base64\r\n".to_string(), wrapped)
}

/// DATA 载荷装配（BUG-20261112-111③）：先裸 LF/CR 归一为 CRLF，再点填充。
/// RFC 5321 会话行终止恒为 CRLF，点填充只覆盖 CRLF-dot——正文中的裸 LF
/// 行首点不被填充，宽容解析裸 LF 的中继（SMTP smuggling 类）可把它当数据
/// 终止、后续字节当新命令（冒认证身份发信）。归一后点填充覆盖全部行首。
pub fn prepare_smtp_payload(message: &str) -> String {
    let normalized = message
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    normalized.replace("\r\n.", "\r\n..")
}

/// rustls ClientConfig 装配（ring provider；显式 CA 信任根，mqtt 同模板
/// ——CA PEM 解析失败/空文件响亮拒绝，不静默降级为无信任根）。
/// 错误以 `String` 返回（crate 级 doc 错误约定：消费方包装本族错误）。
pub fn tls_client_config(ca_pem: &[u8]) -> Result<rustls::ClientConfig, String> {
    use rustls::RootCertStore;
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;

    let mut roots = RootCertStore::empty();
    let ca_certs: Vec<CertificateDer> = CertificateDer::pem_slice_iter(ca_pem)
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| format!("email CA 证书 PEM 解析失败: {e}"))?;
    if ca_certs.is_empty() {
        return Err("email CA 证书文件不含有效 PEM 证书".to_string());
    }
    for cert in ca_certs {
        roots
            .add(cert)
            .map_err(|e| format!("email CA 证书加入信任库失败: {e}"))?;
    }

    let provider = rustls::crypto::ring::default_provider();
    Ok(rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("email TLS 协议版本装配失败: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// rustls ServerConfig 装配（服务端证书链 + 私钥 PEM；SMTP STARTTLS / POP3S
/// 服务端同源消费——与 [`tls_client_config`] 对称）。证书/私钥解析失败或
/// 不匹配响亮拒绝（fail-closed——调用方拒绝启动对应服务面）。
/// 错误以 `String` 返回（crate 级 doc 错误约定：消费方包装本族错误）。
pub fn tls_server_config(cert_pem: &[u8], key_pem: &[u8]) -> Result<rustls::ServerConfig, String> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::PrivateKeyDer;
    use rustls::pki_types::pem::PemObject;

    let certs: Vec<CertificateDer> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| format!("服务端证书 PEM 解析失败: {e}"))?;
    if certs.is_empty() {
        return Err("服务端证书文件不含有效 PEM 证书".to_string());
    }
    let key = PrivateKeyDer::pem_slice_iter(key_pem)
        .next()
        .ok_or("服务端私钥文件不含有效 PEM 私钥")?
        .map_err(|e| format!("服务端私钥 PEM 解析失败: {e}"))?;
    let provider = rustls::crypto::ring::default_provider();
    rustls::ServerConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("email TLS 协议版本装配失败: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("服务端证书/私钥不匹配: {e}"))
}

/// 按「首个冒号」切出 RFC 5322 头字段 (小写字段名, 值)（BUG-20261112-111②）：
/// 字段名大小写不敏感（RFC 5322 §2.2），判定与截取必须同源——此前判定用
/// 小写 starts_with、截取用精确大小写 `trim_start_matches("Message-ID:")`，
/// 「Message-Id:」（实务同义拼写）命中判定后截取失败 → 值整行污染（字段
/// 名混入去重键）；小写「from:」无尖括号形态则 chat_id 被污染 →
/// allow-list 误拦 / 回信投递地址错误。
///
/// pub 面：plugin-mail MailStore 的头提取同源消费（store.rs `extract_header`）。
pub fn split_header_field(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once(':')?;
    Some((name.trim().to_lowercase(), value.trim().to_string()))
}

/// 头字段写入模式（BUG-20261112-111② 语义保真）：直接 untagged 行 =
/// 后写覆盖（原语义）；literal 头数据 = 首个命中保留（原语义）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldWriteMode {
    Overwrite,
    KeepExisting,
}

/// 解析一行头字段并按模式写入目标槽（字段名/值同源切分，message-id 额外
/// 剥尖括号与空白）。
fn apply_header_field(
    line: &str,
    from: &mut String,
    subject: &mut String,
    message_id: &mut String,
    mode: FieldWriteMode,
) {
    let Some((name, value)) = split_header_field(line) else {
        return;
    };
    let write = |slot: &mut String, v: &str| match mode {
        FieldWriteMode::Overwrite => *slot = v.to_string(),
        FieldWriteMode::KeepExisting => {
            if slot.is_empty() {
                *slot = v.to_string();
            }
        }
    };
    match name.as_str() {
        "from" => write(from, &value),
        "subject" => write(subject, &value),
        "message-id" => write(
            message_id,
            value.trim_matches(|c| c == '<' || c == '>' || c == ' '),
        ),
        _ => {}
    }
}

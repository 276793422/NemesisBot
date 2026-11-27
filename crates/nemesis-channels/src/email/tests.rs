use super::*;

/// 构造测试用总线发送端（broadcast::channel 是同步构造，不依赖 runtime）。
fn test_sender() -> tokio::sync::broadcast::Sender<InboundMessage> {
    tokio::sync::broadcast::channel(16).0
}

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
    assert_eq!(EmailChannel::build_reply_subject("Hello"), "Re: Hello");
    assert_eq!(EmailChannel::build_reply_subject("Re: Hello"), "Re: Hello");
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
    assert!(EmailChannel::new(config, test_sender()).is_err());
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
    let ch = EmailChannel::new(config, test_sender()).unwrap();
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
    let ch = EmailChannel::new(config, test_sender()).unwrap();

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
    let (from, subject, message_id, _cte) = EmailChannel::parse_email_headers(&responses);
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
    let body = EmailChannel::parse_email_body(&responses, "");
    assert_eq!(body, "Hello world");
}

#[test]
fn test_parse_email_body_empty() {
    let body = EmailChannel::parse_email_body(&[], "");
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
    let ch = EmailChannel::new(config, test_sender()).unwrap();
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
    let responses = vec!["NB00 OK SEARCH completed".to_string()];
    let nums = EmailChannel::parse_search_results(&responses);
    assert!(nums.is_empty());
}

#[test]
fn test_parse_search_results_single() {
    let responses = vec!["* SEARCH 42".to_string(), "NB00 OK".to_string()];
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
    let (from, subject, message_id, _cte) = EmailChannel::parse_email_headers(&[]);
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
    let body = EmailChannel::parse_email_body(&responses, "");
    assert!(body.contains("Line one"));
    assert!(body.contains("Line two"));
}

// ---- BUG-20261127-043：收件对称臂（RFC 2047 subject 解码 + base64 CTE 体解码）
// 修复前：出站编码（BUG-20261118-130 两处）而无入站解码——自产邮件自己收回来
// subject 是 encoded-word 裸串、正文是 base64 载荷，且头 Fetch 命令根本不取
// CONTENT-TRANSFER-ENCODING（收端无从得知编码）。

#[test]
fn test_decode_rfc2047_single_word_and_passthrough() {
    assert_eq!(
        nemesis_mail_protocol::decode_rfc2047("=?UTF-8?B?5L2g5aW9?="),
        "你好"
    );
    // 无 encoded-word 纯文本零漂移
    assert_eq!(
        nemesis_mail_protocol::decode_rfc2047("Plain Subject"),
        "Plain Subject"
    );
}

#[test]
fn test_decode_rfc2047_roundtrip_multibyte_and_chunked() {
    for subject in [
        "你好",
        "混合 text 与中文 subject 行",
        "多字节截断边界测试：这个主题字符串足够长，编码后 base64 载荷会超过 48 字符而切成多块 encoded-word 序列，解码必须还原原文（相邻词间 CRLF+WSP 空白按 RFC 2047 §2 丢弃）",
    ] {
        let enc = super::encode_rfc2047_subject(subject);
        assert_eq!(
            nemesis_mail_protocol::decode_rfc2047(&enc),
            subject,
            "round-trip 失败: {subject}"
        );
    }
}

#[test]
fn test_decode_rfc2047_mixed_text_keeps_separating_whitespace() {
    // encoded-word 与普通文本之间的空白保留（只有相邻 encoded-word 间的空白才丢）
    assert_eq!(
        nemesis_mail_protocol::decode_rfc2047("Hi =?UTF-8?B?5L2g5aW9?=!"),
        "Hi 你好!"
    );
}

#[test]
fn test_decode_rfc2047_unsupported_forms_kept_raw() {
    // 非 B 编码 / 非 UTF-8 charset / 载荷损坏 / 无闭合 → 原样保留（诚实降级，
    // 不猜码表不猜 mojibake）
    let raw_q = "=?UTF-8?Q?hello?=";
    assert_eq!(nemesis_mail_protocol::decode_rfc2047(raw_q), raw_q);
    let raw_gbk = "=?GBK?B?5L2g5aW9?=";
    assert_eq!(nemesis_mail_protocol::decode_rfc2047(raw_gbk), raw_gbk);
    let corrupt = "x =?UTF-8?B?!!!?= y";
    assert_eq!(
        nemesis_mail_protocol::decode_rfc2047(corrupt),
        "x =?UTF-8?B?!!!?= y"
    );
    let unclosed = "a =?UTF-8?B? abc";
    assert_eq!(nemesis_mail_protocol::decode_rfc2047(unclosed), unclosed);
}

#[test]
fn test_parse_email_headers_extracts_cte_and_decodes_subject() {
    let responses = vec![
        "* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID CONTENT-TRANSFER-ENCODING)] {130}"
            .to_string(),
        "From: Alice <alice@example.com>".to_string(),
        "Subject: =?UTF-8?B?5L2g5aW9?=".to_string(),
        "Message-ID: <m2@example.com>".to_string(),
        "Content-Transfer-Encoding: base64".to_string(),
        ")".to_string(),
    ];
    let (from, subject, message_id, cte) = EmailChannel::parse_email_headers(&responses);
    assert_eq!(from, "Alice <alice@example.com>");
    assert_eq!(subject, "你好");
    assert_eq!(message_id, "m2@example.com");
    assert_eq!(cte, "base64");
}

#[test]
fn test_parse_email_body_base64_roundtrip_incl_76_wrap() {
    let body = "你好，世界——收件对称臂 round-trip 验证用正文，长度要足够触发 76 字符折行的多行形态，确保剥空白归一覆盖 CRLF 行切分。";
    let (cte_line, encoded) = super::encode_body_mime(body);
    assert!(cte_line.contains("base64"));
    // 模拟 IMAP read_literal_lines 的行切分形态（76 字符 CRLF 折行 → 多行）
    let lines: Vec<String> = encoded.split("\r\n").map(|s| s.to_string()).collect();
    assert!(lines.len() > 1, "fixture 应为多行折行形态");
    assert_eq!(EmailChannel::parse_email_body(&lines, "base64"), body);
}

#[test]
fn test_parse_email_body_base64_decode_failure_note() {
    let responses = vec![
        "* 1 FETCH (BODY[TEXT] {4}".to_string(),
        "!!!!".to_string(),
        ")".to_string(),
    ];
    let body = EmailChannel::parse_email_body(&responses, "base64");
    assert!(body.contains("base64 解码失败"), "got: {body}");
    assert!(body.contains("!!!!"), "解码失败必须保留原文");
}

#[test]
fn test_email_roundtrip_nonascii_body_and_subject() {
    // 端到端对称锁：build_smtp_message 产物 → 头段/BODY[TEXT] 段 →
    // parse_email_headers / parse_email_body 还原原文。
    let subject = "构建通知：中文标题";
    let body = "第一行内容\n第二行内容";
    let msg = EmailChannel::build_smtp_message("a@b.com", "c@d.com", subject, body);
    let (headers, payload) = msg.split_once("\r\n\r\n").expect("header/body 分隔");
    assert!(headers.contains("Content-Transfer-Encoding: base64"));
    assert!(headers.contains("Subject: =?UTF-8?B?"));
    // BODY[TEXT] = 头区之后的载荷（read_literal_lines 按行切分形态）
    let lines: Vec<String> = payload.split("\r\n").map(|s| s.to_string()).collect();
    assert_eq!(EmailChannel::parse_email_body(&lines, "base64"), body);
    let header_lines: Vec<String> = headers.split("\r\n").map(|s| s.to_string()).collect();
    let (_, got_subject, _, cte) = EmailChannel::parse_email_headers(&header_lines);
    assert_eq!(got_subject, subject);
    assert_eq!(cte, "base64");
}

#[test]
fn test_build_smtp_message_content() {
    let msg = EmailChannel::build_smtp_message(
        "sender@test.com",
        "receiver@test.com",
        "Test",
        "Body content",
    );
    assert!(msg.contains("Content-Type: text/plain; charset=UTF-8"));
    assert!(msg.contains("Body content"));
}

#[test]
fn test_email_config_default() {
    let cfg = EmailConfig::default();
    assert!(cfg.imap_host.is_empty());
    assert!(cfg.smtp_host.is_empty());
    assert!(cfg.imap_username.is_empty());
    assert!(cfg.imap_password.is_empty());
    assert_eq!(cfg.poll_interval, 30);
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
    let ch = EmailChannel::new(config, test_sender()).unwrap();

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
    let ch = EmailChannel::new(config, test_sender()).unwrap();
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
    let ch = EmailChannel::new(config, test_sender()).unwrap();
    assert_eq!(ch.smtp_password(), "smtp_pass");
}

// ---- （审计修复）：SMTP/IMAP IO 超时 ----------------------------

#[tokio::test(start_paused = true)]
async fn test_io_timeout_pending_future_times_out() {
    // 挂起 future 必须按时（虚拟时钟）返回 TimedOut，错误文案带时长。
    let started = std::time::Instant::now();
    let r = io_timeout(
        "unit read",
        std::time::Duration::from_secs(IO_TIMEOUT_SECS),
        std::future::pending::<std::io::Result<usize>>(),
    )
    .await;
    let err = r.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(err.to_string().contains("timed out after 30s"));
    // 虚拟时钟：不真等 30s 墙钟。
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "must not really block 30s, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn test_io_timeout_passthrough_ok_and_err() {
    // 未超时：底层 Ok/Err 原样穿透，语义不被包装改变。
    let ok = io_timeout(
        "ok",
        std::time::Duration::from_secs(IO_TIMEOUT_SECS),
        std::future::ready(Ok::<_, std::io::Error>(42)),
    )
    .await;
    assert_eq!(ok.unwrap(), 42);

    let e = io_timeout(
        "err",
        std::time::Duration::from_secs(IO_TIMEOUT_SECS),
        std::future::ready(Err::<usize, _>(std::io::Error::other("boom"))),
    )
    .await;
    assert_eq!(e.unwrap_err().to_string(), "boom");
}

#[tokio::test(start_paused = true)]
async fn test_smtp_send_black_hole_times_out() {
    // 本地黑洞 mock：accept 后永不回 greeting——修复前 smtp_send 读永久挂死
    // （dispatch_loop 串行 → 全通道出站停摆）；修复后 30s 虚拟时钟超时，
    // 诚实 Err 返回。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // 持有 accept 到的连接（不读不写不关——黑洞；提前 drop 会 EOF，
        // 走「unexpected greeting」而非挂起路径）。
        let (_stream, _) = listener.accept().await.unwrap();
        // sleep 仅为持有资源到测试结束（其 3600s 虚拟期限晚于读超时的
        // 30s，不影响判定顺序）。
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    });

    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        smtp_host: addr.ip().to_string(),
        smtp_port: addr.port(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        ..Default::default()
    };
    let ch = EmailChannel::new(config, test_sender()).unwrap();

    let started = std::time::Instant::now();
    let err = ch
        .smtp_send("to@example.com", "subject", "body")
        .await
        .expect_err("black-hole server must produce a timeout error, not hang");
    let msg = format!("{err}");
    assert!(
        msg.contains("timed out after 30s"),
        "expect IO timeout error, got: {msg}"
    );
    // 虚拟时钟：30s 超时瞬间走完，dispatch_loop 不会被拖死。
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "must not really block 30s, took {:?}",
        started.elapsed()
    );
}

// start() 把 sender_map 交给 IMAP 轮询任务的必须是共享
// 克隆——裸 DashMap::clone() 是逐 shard 深拷贝，任务侧写入的回复线索
// （chat_id -> 发件人/主题）落私有副本，send() 的读路径（self.sender_map
// .get）读原表恒 miss，邮件回复线程化静默失效（Re: 主题恒落默认值）。
// 本测试复刻任务侧捕获形态（与字段同源.clone()）写入后，断言结构体侧
// 读路径可见——旧实现（裸 DashMap）红，Arc 化后绿。
#[test]
fn test_sender_map_shared_between_task_clone_and_read_path() {
    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        ..Default::default()
    };
    let ch = EmailChannel::new(config, test_sender()).unwrap();

    // 复刻 start() 542 行的捕获形态：与字段同源克隆交给「任务侧」。
    let task_side_map = ch.sender_map.clone();
    task_side_map.insert(
        "chat-1".to_string(),
        EmailMessageInfo {
            sender_email: "alice@example.com".to_string(),
            subject: "Re: Hello".to_string(),
        },
    );

    // 结构体侧读路径（send() 859 行同源表达式）：必须看到任务侧写入。
    assert_eq!(
        ch.sender_map.get("chat-1").map(|info| info.subject.clone()),
        Some("Re: Hello".to_string()),
        "reply info written via the task-side clone must be visible to the send() read path (BUG-20261029-142)"
    );
}

// ----：收件黑洞（IMAP 轮询到标已读为止，从不发布总线） ----

/// 起一个只讲最小 IMAP 方言的假服务器（单连接）：greeting → LOGIN/SELECT OK →
/// SEARCH 返回 1 封 → FETCH 头（Alice / Hello there / m1）→ FETCH 正文
/// （"Hello body"）→ STORE/LOGOUT OK。命令匹配按内容不按 tag，tag 响应号
/// 与客户端 next_tag 顺序（..）对齐。
async fn spawn_fake_imap() -> std::net::SocketAddr {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let (rd, mut wr) = stream.into_split();
        let mut rd = BufReader::new(rd);
        let mut line = String::new();
        if wr.write_all(b"* OK IMAP4rev1 ready\r\n").await.is_err() {
            return;
        }
        loop {
            line.clear();
            if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let resp = if line.contains("LOGIN") {
                "NB001 OK LOGIN\r\n".to_string()
            } else if line.contains("SELECT") {
                "NB002 OK SELECT\r\n".to_string()
            } else if line.contains("SEARCH") {
                "* SEARCH 1\r\nNB003 OK SEARCH\r\n".to_string()
            } else if line.contains("BODY[HEADER") {
                "* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {85}\r\n\
                 From: Alice <alice@example.com>\r\n\
                 Subject: Hello there\r\n\
                 Message-ID: <m1@example.com>\r\n\
                 )\r\n\
                 NB004 OK FETCH\r\n"
                    .to_string()
            } else if line.contains("BODY[TEXT]") {
                "* 1 FETCH (BODY[TEXT] {10}\r\nHello body\r\n)\r\nNB005 OK FETCH\r\n".to_string()
            } else if line.contains("STORE") {
                "NB006 OK STORE\r\n".to_string()
            } else if line.contains("LOGOUT") {
                "NB007 OK LOGOUT\r\n".to_string()
            } else {
                "NB999 OK\r\n".to_string()
            };
            if wr.write_all(resp.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    addr
}

/// 修复前红证形态：旧代码轮询循环把邮件处理到「标已读 + debug 日志」为止，
/// `_body` 取了即丢、构造器没有 bus 参数——总线订阅端永远收不到任何消息
/// （本测试在旧源码下连编译都过不了：`EmailChannel::new` 只收 1 个参数，
/// 收件发布面在修复前结构性不存在）。修复后：轮询收到邮件必须以
/// InboundMessage 发布到总线，content=正文、metadata 带 subject、
/// chat_id=发件人地址（回复可路由）。
#[tokio::test]
async fn test_poll_loop_publishes_inbound_email_to_bus() {
    let addr = spawn_fake_imap().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let config = EmailConfig {
        imap_host: addr.ip().to_string(),
        imap_port: addr.port(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        poll_interval: 3600, // 首 tick 立即触发，测试期内不再询
        ..Default::default()
    };
    let ch = EmailChannel::new(config, tx).unwrap();
    ch.start().await.unwrap();

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("poll loop must publish the fetched email within 5s")
        .unwrap();
    assert_eq!(inbound.channel, "email");
    assert_eq!(inbound.chat_id, "alice@example.com");
    assert_eq!(inbound.sender_id, "alice@example.com");
    assert_eq!(inbound.content, "Hello body");
    assert_eq!(
        inbound.metadata.get("subject").map(String::as_str),
        Some("Hello there")
    );

    ch.stop().await.unwrap();
}

/// allow_from 白名单必须真正拦截（旧代码 allow_from 全文件零消费）：
/// 白名单只放行 bob 时，alice 的邮件不得进总线。
#[tokio::test]
async fn test_poll_loop_blocks_disallowed_sender() {
    let addr = spawn_fake_imap().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let config = EmailConfig {
        imap_host: addr.ip().to_string(),
        imap_port: addr.port(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        poll_interval: 3600,
        allow_from: vec!["bob@example.com".to_string()],
        ..Default::default()
    };
    let ch = EmailChannel::new(config, tx).unwrap();
    ch.start().await.unwrap();

    let nothing = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
    assert!(
        nothing.is_err(),
        "disallowed sender must not reach the bus, got: {nothing:?}"
    );

    ch.stop().await.unwrap();
}

// ---- BUG-20261108-028：外部流行读无帽（IMAP 位点回归锁） ----

/// 起一个在 SEARCH 响应里吐毒丸行的假 IMAP 服务器（骨架同 spawn_fake_imap）：
/// SEARCH 响应 = 一行 8MB+1KB 无换行毒丸 + 正常结果——轮询必须带帽截断
/// 续收（游标对齐、连接保活），后续 FETCH/STORE 照常完成。
async fn spawn_fake_imap_with_poison() -> std::net::SocketAddr {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let (rd, mut wr) = stream.into_split();
        let mut rd = BufReader::new(rd);
        let mut line = String::new();
        if wr.write_all(b"* OK IMAP4rev1 ready\r\n").await.is_err() {
            return;
        }
        loop {
            line.clear();
            if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let resp = if line.contains("LOGIN") {
                "NB001 OK LOGIN\r\n".to_string()
            } else if line.contains("SELECT") {
                "NB002 OK SELECT\r\n".to_string()
            } else if line.contains("SEARCH") {
                let mut resp = "x".repeat(8 * 1024 * 1024 + 1024);
                resp.push_str("\r\n* SEARCH 1\r\nNB003 OK SEARCH\r\n");
                resp
            } else if line.contains("BODY[HEADER") {
                "* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {85}\r\n\
                 From: Alice <alice@example.com>\r\n\
                 Subject: Hello there\r\n\
                 Message-ID: <m1@example.com>\r\n\
                 )\r\n\
                 NB004 OK FETCH\r\n"
                    .to_string()
            } else if line.contains("BODY[TEXT]") {
                "* 1 FETCH (BODY[TEXT] {10}\r\nHello body\r\n)\r\nNB005 OK FETCH\r\n".to_string()
            } else if line.contains("STORE") {
                "NB006 OK STORE\r\n".to_string()
            } else if line.contains("LOGOUT") {
                "NB007 OK LOGOUT\r\n".to_string()
            } else {
                "NB999 OK\r\n".to_string()
            };
            if wr.write_all(resp.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    addr
}

/// 毒丸行（>8MB 帽）不杀死轮询：SEARCH 阶段截断续收后协议游标对齐，
/// 邮件照常取到并发布——修复前裸 read_line 把毒丸无界积累进内存（OOM 面，
/// 帽内截断会烧 8MB×每毒丸行但受界）。带帽的语义级断言 = 轮询在毒丸后
/// 仍完成整链（SEARCH→FETCH→STORE→发布），5s 内拿到 InboundMessage。
#[tokio::test]
async fn test_poll_loop_survives_oversized_line() {
    let addr = spawn_fake_imap_with_poison().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let config = EmailConfig {
        imap_host: addr.ip().to_string(),
        imap_port: addr.port(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        poll_interval: 3600,
        ..Default::default()
    };
    let ch = EmailChannel::new(config, tx).unwrap();
    ch.start().await.unwrap();

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("poll must survive the poison line and publish the email within 5s")
        .unwrap();
    assert_eq!(inbound.content, "Hello body");

    ch.stop().await.unwrap();
}

/// 加固臂：build_smtp_message 是 pub 编码单一入口——from/to/subject 混入
/// CR/LF 必须被拍平成单行（RFC 5322 头部值形态），注入的「头」退化为
/// Subject 值内的普通文本；body 区不受影响（dot-stuffing 的辖区）。
#[test]
fn test_build_smtp_message_flattens_crlf_in_headers() {
    let msg = EmailChannel::build_smtp_message(
        "bot@example.com",
        "user@example.com",
        "Hi\r\nBcc: victim@example.com",
        "line1\r\n.line2",
    );
    assert!(
        msg.starts_with(
            "From: bot@example.com\r\n\
             To: user@example.com\r\n\
             Subject: HiBcc: victim@example.com\r\n\
             Date: "
        ),
        "header CR/LF must be stripped inlined, got: {msg}"
    );
    assert!(
        !msg.contains("\r\nBcc:"),
        "injected header line must not exist, got: {msg}"
    );
    assert!(
        msg.ends_with("line1\r\n.line2"),
        "body region must pass through untouched, got: {msg}"
    );
}

/// BUG-20261108-021 回归锁：seen_uids 去重图容量帽。
/// 此前 mark_seen 与轮询内联插入只增不删——键为机器键（每封收件
/// Message-ID），长驻进程随收件量线性增长永不回收 = 内存慢漏。
/// 家族先例（mastodon/nostr/signal 10000→5000 修剪）对齐后：
/// 超帽必须修剪有界（本测 11000 次插入后断言 ≤ 10000）。
#[test]
fn test_seen_uids_capped_no_unbounded_growth() {
    let config = EmailConfig {
        imap_host: "imap.test.local".to_string(),
        smtp_host: "smtp.test.local".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        ..Default::default()
    };
    let channel = EmailChannel::new(config, test_sender()).expect("channel construct");
    let total = EmailChannel::SEEN_UIDS_MAX_ENTRIES + 1000;
    for i in 0..total {
        channel.mark_seen(&format!("<cap-{i}@test.local>"));
    }
    let len = channel.seen_uids.read().len();
    assert!(
        len <= EmailChannel::SEEN_UIDS_MAX_ENTRIES,
        "seen_uids must stay capped at {}, got {len} after {total} inserts",
        EmailChannel::SEEN_UIDS_MAX_ENTRIES
    );
    assert!(len > 0, "prune must not drain the map to empty");
}

// ---- R737 协议注入面回归锁 ----

#[test]
fn test_r737_parse_search_results_rejects_non_numeric_tokens() {
    // IMAP 服务器（或 MITM）返回畸形 token：不得原样进 STORE/FETCH 命令流。
    let responses = vec![
        "* SEARCH 1 2evil 3x,y".to_string(),
        "* SEARCH".to_string(),
        "* SEARCH 7".to_string(),
    ];
    let nums = EmailChannel::parse_search_results(&responses);
    assert_eq!(nums, vec!["1", "7"], "非数字 token 必须丢弃");
}

#[test]
fn test_r737_imap_quoted_escapes_protocol_metachars() {
    assert_eq!(super::imap_quoted("INBOX"), "\"INBOX\"");
    // `"` 与 `\` 用 `\` 转义（RFC 3501 quoted string）。
    assert_eq!(super::imap_quoted(r#"a"b\c"#), "\"a\\\"b\\\\c\"");
    // CR/LF 剥除——quoted 内不允许换行，含 CRLF 的密码/文件夹名不得破坏
    // 协议流（注入后续命令）。
    assert_eq!(
        super::imap_quoted("bad\r\nSELECT \"x\""),
        "\"badSELECT \\\"x\\\"\""
    );
}

#[test]
fn test_r737_smtp_envelope_safe_strips_crlf() {
    assert_eq!(super::smtp_envelope_safe("a@b.com"), "a@b.com");
    // envelope 值夹带 CRLF = DATA 前命令注入，必须剥除。
    assert_eq!(
        super::smtp_envelope_safe("victim@x.com>\r\nRCPT TO:<spam@y.com"),
        "victim@x.com>RCPT TO:<spam@y.com"
    );
    assert_eq!(super::smtp_envelope_safe("a\rb@c\nd.com"), "ab@cd.com");
}

// ----：TLS 接线（2026-11-11）与 poll_interval 收口 ----

/// TLS 显式信任根 fail-closed（mqtt 同原则）：任一 tls=true 而无
/// ca_cert_path = 构造期响亮拒绝；tls=false 不要求 CA（明文形态不受影响）。
#[test]
fn tls_without_explicit_ca_root_is_rejected() {
    let base = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        ..Default::default()
    };

    let imap_tls = EmailConfig {
        imap_tls: true,
        ..base.clone()
    };
    let err = EmailChannel::new(imap_tls, test_sender())
        .err()
        .expect("imap_tls=true without ca_cert_path must be rejected at construction");
    assert!(format!("{err}").contains("ca_cert_path"), "got: {err}");

    let smtp_tls = EmailConfig {
        smtp_tls: true,
        ..base.clone()
    };
    let err = EmailChannel::new(smtp_tls, test_sender())
        .err()
        .expect("smtp_tls=true without ca_cert_path must be rejected at construction");
    assert!(format!("{err}").contains("ca_cert_path"), "got: {err}");

    // 配了 CA 后两臂都可通过构造。
    let both_tls = EmailConfig {
        imap_tls: true,
        smtp_tls: true,
        ca_cert_path: "/tmp/ca.pem".to_string(),
        ..base
    };
    EmailChannel::new(both_tls, test_sender()).expect("tls=true with ca_cert_path must construct");
}

/// poll_interval 收口：0 回落默认 30（tokio interval 零周期 panic 的
/// 防线），非零原样透传。
#[test]
fn effective_poll_interval_zero_falls_back_to_default() {
    assert_eq!(EmailChannel::effective_poll_interval(0), 30);
    assert_eq!(EmailChannel::effective_poll_interval(1), 1);
    assert_eq!(EmailChannel::effective_poll_interval(30), 30);
    assert_eq!(EmailChannel::effective_poll_interval(600), 600);
}

/// CA 文件缺失/不可读 = 诚实错误（不静默降级明文）：tls_wrap 在读 CA
/// 处即失败，错误带路径上下文。
#[tokio::test]
async fn tls_wrap_missing_ca_file_is_honest_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    // 先建一条真 TCP 连接（tls_wrap 的入参前提），服务器侧黑洞持有即可。
    let server = tokio::spawn(async move {
        let (_s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    let tcp = tokio::net::TcpStream::connect(server_addr).await.unwrap();

    let err = tls_wrap("IMAP", "127.0.0.1", "/nonexistent/nemesis_ca.pem", tcp)
        .await
        .err()
        .expect("missing CA file must fail loudly");
    let msg = format!("{err}");
    assert!(
        msg.contains("CA 证书读取失败") && msg.contains("/nonexistent/nemesis_ca.pem"),
        "error must carry CA path context, got: {msg}"
    );
    server.abort();
}

// ---- BUG-20261112-111：email 协议解析/编码正确性三面 ----

/// ① tagged 完成行状态按位置取 token：`contains("OK")` 会把 NO/BAD 文本
/// 恰含大写 OK 词素的行（BROKEN/TOKEN/BOOKMARK 类服务器文案）误判成功。
#[test]
fn test_tagged_line_status_111() {
    // 完成行（本 tag）：
    assert!(tagged_line_status("NB001 OK", "NB001").unwrap());
    assert!(tagged_line_status("NB001 OK Done", "NB001").unwrap());
    // NO/BAD 完成行——含大写 OK 词素的文本此前全部误判成功：
    assert_eq!(
        tagged_line_status("NB001 NO Broken credentials", "NB001"),
        Some(false)
    );
    assert_eq!(
        tagged_line_status("NB001 NO [SERVERBUG] Bad TOKEN", "NB001"),
        Some(false)
    );
    assert_eq!(
        tagged_line_status("NB001 NO Mailbox BOOKMARKED", "NB001"),
        Some(false)
    );
    assert_eq!(
        tagged_line_status("NB001 BAD Command stream BROKEN", "NB001"),
        Some(false)
    );
    // 非 completion 行（继续收集）：
    assert_eq!(tagged_line_status("* 1 FETCH (...) {68}", "NB001"), None);
    assert_eq!(tagged_line_status("* SEARCH 1 2 3", "NB001"), None);
    assert_eq!(
        tagged_line_status("NB0012 OK", "NB001"),
        None,
        "更长 tag 同前缀非本完成行"
    );
    assert_eq!(
        tagged_line_status("NB001OK", "NB001"),
        None,
        "无 SP 分隔非完成行"
    );
}

/// ② 头字段判定与截取同源（小写比较 + 冒号切分）：「Message-Id:」实务
/// 同义拼写此前被整行污染；小写「from:」无尖括号形态污染 chat_id。
#[test]
fn test_parse_email_headers_case_insensitive_111() {
    // Message-Id（小写 d，RFC 5322 注册拼写的常见实务变体）：
    let responses = vec![
        "* 1 FETCH (... {40}".to_string(),
        "Message-Id: <mid42@example.com>".to_string(),
        "From: Bob <bob@example.com>".to_string(),
        ")".to_string(),
    ];
    let (from, _subject, message_id, _cte) = EmailChannel::parse_email_headers(&responses);
    assert_eq!(
        message_id, "mid42@example.com",
        "字段名不得混入值: {message_id}"
    );
    assert_eq!(from, "Bob <bob@example.com>");

    // 小写 from + 无尖括号（此前 chat_id = 整行 → allow-list 误拦/回信错址）：
    let responses = vec![
        "* 1 FETCH (... {20}".to_string(),
        "from: carol@example.com".to_string(),
        "SUBJECT: Uppercase Name".to_string(),
        ")".to_string(),
    ];
    let (from, subject, _, _cte) = EmailChannel::parse_email_headers(&responses);
    assert_eq!(from, "carol@example.com");
    assert_eq!(subject, "Uppercase Name");

    // 值含冒号（Subject: Re: xxx）——按首个冒号切分不截断：
    let responses = vec!["Subject: Re: Hello: World".to_string()];
    let (_, subject, _, _cte) = EmailChannel::parse_email_headers(&responses);
    assert_eq!(subject, "Re: Hello: World");

    // 无空格冒号形态 Message-ID:<x>（此前 trim 后括号残留路径）：
    let responses = vec!["Message-ID:<nospace@example.com>".to_string()];
    let (_, _, message_id, _cte) = EmailChannel::parse_email_headers(&responses);
    assert_eq!(message_id, "nospace@example.com");
}

/// ③ DATA 载荷装配：裸 LF/CR 归一 CRLF 后点填充覆盖全部行首——裸 LF
/// 「.\n」不再可作数据终止（SMTP smuggling 类）。
#[test]
fn test_prepare_smtp_payload_111() {
    // 裸 LF 行首点：归一 + 填充（此前原样穿透 = 终止注入面）：
    assert_eq!(
        prepare_smtp_payload("line1\n.\nline2"),
        "line1\r\n..\r\nline2"
    );
    // smuggling 载荷：正文注入「\n.\nQUIT」被填充为数据行：
    let out = prepare_smtp_payload("body\n.\nMAIL FROM:<evil@x>\r\n");
    assert!(out.contains("\r\n..\r\n"), "裸 LF 点行必须被填充: {out:?}");
    assert!(!out.contains("\n.\n"), "不得残留裸 LF 行首点: {out:?}");
    // 既有 CRLF 点行行为不变：
    assert_eq!(prepare_smtp_payload("a\r\n.b\r\nc"), "a\r\n..b\r\nc");
    // 裸 CR（旧 Mac 形态）同归一：
    assert_eq!(prepare_smtp_payload("x\r.y"), "x\r\n..y");
    // 无点纯 LF 正文仅归一：
    assert_eq!(prepare_smtp_payload("p\nq"), "p\r\nq");
    // 干净载荷不变：
    assert_eq!(
        prepare_smtp_payload("From: a\r\n\r\nhello"),
        "From: a\r\n\r\nhello"
    );
}

// ---- BUG-20261113-126：IMAP literal 字节数核算（假 tag 毒丸失步根修） ----

/// literal 尾标解析形态表：行尾 `{N}` 纯数字合法；空体/超长/非数字/非行尾
/// 一律 None（非 literal）。
#[test]
fn test_trailing_literal_size_shapes() {
    assert_eq!(
        trailing_literal_size("* 1 FETCH (BODY[TEXT] {10}"),
        Some(10)
    );
    assert_eq!(trailing_literal_size("x {0}"), Some(0));
    // 空体 `{}`：None
    assert_eq!(trailing_literal_size("* 1 FETCH (BODY[] {}"), None);
    // 11 位数字：溢出防护直接 None
    assert_eq!(
        trailing_literal_size("* 1 FETCH (BODY[] {99999999999}"),
        None
    );
    // 非纯数字：None
    assert_eq!(trailing_literal_size("* 1 FETCH (BODY[] {12a}"), None);
    // 非行尾：None
    assert_eq!(trailing_literal_size("{10} tail"), None);
    assert_eq!(trailing_literal_size("plain line"), None);
}

/// 核心回归锁：literal 内容里恰为 `{tag} OK ...` 形态的毒丸行**不得**
/// 终止响应收集——修复前 tagged_line_status 直接吃掉毒丸行（提前 Ok），
/// 后续 literal 残字被当协议行连锁失步（STORE 对不上下游命令乱串）。
/// 修复后 literal 按声明字节数精确消费，毒丸行降级为普通 literal 内容，
/// 收集在真 tagged 行终止、游标对齐（下一条 STORE 干净往返为证）。
#[tokio::test]
async fn test_literal_fake_tag_line_does_not_terminate_collection() {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (rd, wr) = tokio::io::split(EmailStream::Test(client));
    let mut conn = ImapConnection {
        reader: BufReader::new(rd),
        writer: wr,
        tag_counter: 0,
    };
    // literal 内嵌假完成行（NB001 = 本命令真 tag，可预测）；字节数如实声明。
    let literal =
        "From: Mallory <mallory@example.com>\r\nNB001 OK PWN\r\nMessage-ID: <m2@example.com>\r\n";
    let resp = format!(
        "* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {{{}}}\r\n{}\r\n)\r\nNB001 OK FETCH\r\n",
        literal.len(),
        literal
    );
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let mut srv = BufReader::new(server);
        srv.write_all(resp.as_bytes()).await.unwrap();
        srv.flush().await.unwrap();
        // 第一条命令（FETCH）此前已发——收掉。
        let mut cmd = String::new();
        srv.read_line(&mut cmd).await.unwrap();
        // 第二条命令（STORE）只有游标对齐才会以完整一行抵达——失步时
        // 这里读到的是残字碎片，断言 red。
        let mut cmd2 = String::new();
        srv.read_line(&mut cmd2).await.unwrap();
        assert!(
            cmd2.contains("STORE"),
            "protocol desynced: second command arrived as fragment: {cmd2:?}"
        );
        srv.write_all(b"NB002 OK STORE\r\n").await.unwrap();
        srv.flush().await.unwrap();
    });

    let responses = conn
        .send_command_multi("FETCH 1 (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)])")
        .await
        .expect("collection must terminate at the real tagged line");
    // 假 tag 行必须作为 literal 内容在场（收集未被它截断）：
    assert!(
        responses.iter().any(|l| l == "NB001 OK PWN"),
        "fake tag line must be collected as literal content: {responses:?}"
    );
    assert!(
        responses
            .iter()
            .any(|l| l.contains("Message-ID: <m2@example.com>")),
        "literal after the fake tag line must still be collected: {responses:?}"
    );
    // 头解析照常（毒丸不污染字段）：
    let (from, _, message_id, _cte) = EmailChannel::parse_email_headers(&responses);
    assert_eq!(
        EmailChannel::extract_email_address(&from),
        "mallory@example.com"
    );
    assert_eq!(message_id, "m2@example.com");
    // 游标对齐实证：STORE 干净往返。
    conn.send_command("STORE 1 +FLAGS (\\Seen)")
        .await
        .expect("protocol must stay aligned after literal with fake tag line");
}

/// 超大 literal 声明（>64MB 帽）诚实 fail-closed：帽检查先于任何字节读
/// ——不吃字节、不挂起、不无界入内存，Err 断连走下轮重连。
#[tokio::test]
async fn test_literal_over_cap_claim_fails_closed() {
    let (client, mut server) = tokio::io::duplex(4096);
    let (rd, wr) = tokio::io::split(EmailStream::Test(client));
    let mut conn = ImapConnection {
        reader: BufReader::new(rd),
        writer: wr,
        tag_counter: 0,
    };
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        // 只喂标记行（70MB 声明 > 64MB 帽），此后不再喂——若实现先读字节
        // 后查帽，本测试挂死（timeout 兜底红）。
        let _ = server.write_all(b"* 1 FETCH (BODY[] {70000000}\r\n").await;
    });
    let err = conn.send_command_multi("FETCH 1 (BODY[])").await;
    assert!(
        err.is_err(),
        "oversize literal claim must fail closed, got: {err:?}"
    );
}

// ---- BUG-20261113-127：sender_map 回复线索表容量帽 ----

/// 家族纪律对齐（seen_uids 同款）：超帽必须修剪有界——插入点在 allow-list
/// 之前，公开邮箱收垃圾邮件可灌无限唯一地址，无帽 = 长驻进程内存慢漏。
/// 不变量按家族语义（超帽删固定批量，非删到下限）：生产模式（插入后即
/// 修剪）任意时刻 len ≤ MAX+1；删 1000 条独有地址持续超帽仍被压回有界
/// ——无帽实现该断言在第一个超帽插入点即红。
#[test]
fn test_sender_map_capped_no_unbounded_growth() {
    let map: dashmap::DashMap<String, EmailMessageInfo> = dashmap::DashMap::new();
    let cap = EmailChannel::SENDER_MAP_MAX_ENTRIES;
    for i in 0..(cap + 1000) {
        let addr = format!("u{i}@example.com");
        map.insert(
            addr.clone(),
            EmailMessageInfo {
                sender_email: addr,
                subject: "s".to_string(),
            },
        );
        EmailChannel::prune_sender_map(&map);
        assert!(
            map.len() <= cap + 1,
            "sender_map must stay bounded (≤ MAX+1): len={} at i={i}",
            map.len()
        );
    }
    // 修剪确实回收了条目（不是只防上限）：最终远小于灌入总数。
    assert!(
        map.len() < cap + 1000,
        "pruning must actually evict entries: len={}",
        map.len()
    );
}

// ---- BUG-20261113-128：build_imap_login 同族转义（R737 横扫漏收位点） ----

/// 凭据含 `"`/`\`/CRLF 不得破坏 quoted 边界——连接路径（imap_connect /
/// imap_connect_static）已走 imap_quoted，本编码入口同源对齐。
#[test]
fn test_build_imap_login_escapes_quoted_injection() {
    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "use\"r".to_string(),
        imap_password: "pa\\ss\r\nNOPE".to_string(),
        ..Default::default()
    };
    let ch = EmailChannel::new(config, test_sender()).unwrap();
    let login = ch.build_imap_login();
    assert!(login.starts_with("LOGIN "), "got: {login:?}");
    assert!(
        login.contains("use\\\"r"),
        "quote must be escaped: {login:?}"
    );
    assert!(
        login.contains("pa\\\\ss"),
        "backslash must be escaped: {login:?}"
    );
    assert!(
        !login.contains('\r') && !login.contains('\n'),
        "CR/LF must be stripped: {login:?}"
    );
}

// ---- BUG-20261115-015：入站身份键大小写归一（email + irc 同族修复） ----

/// 同一地址的大小写变体必须收敛为同一身份键：原大小写入键会在内存里
/// 分裂成两个会话（chat_id/session_key/sender_map 键各一份），而 NTFS/
/// APFS 文件名不敏感 → 两个 session_key 落同一个磁盘 jsonl 交错互串。
#[test]
fn test_identity_chat_id_case_variants_converge() {
    assert_eq!(EmailChannel::identity_chat_id("User@X.com"), "user@x.com");
    assert_eq!(EmailChannel::identity_chat_id("USER@X.COM"), "user@x.com");
    assert_eq!(
        EmailChannel::identity_chat_id("User@X.com"),
        EmailChannel::identity_chat_id("user@x.com")
    );
    // 空地址哨兵不变
    assert_eq!(EmailChannel::identity_chat_id(""), "email:unknown");
}

/// 回信二元组：命中 sender_map 时 To 用原文（保留对方地址大小写——本地
/// 部分 RFC 5321 名义区分大小写），主题用线索化结果；未命中回落归一化
/// 身份键 + 默认主题（进程重启 sender_map 清空 / bot 主动发起形态）。
#[test]
fn test_reply_target_and_subject_prefers_original_address() {
    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "imap_user".to_string(),
        imap_password: "imap_pass".to_string(),
        ..Default::default()
    };
    let ch = EmailChannel::new(config, test_sender()).unwrap();
    ch.sender_map.insert(
        "user@x.com".to_string(),
        EmailMessageInfo {
            sender_email: "User@X.com".to_string(),
            subject: "Re: Hello".to_string(),
        },
    );
    let (to, subject) = ch.reply_target_and_subject("user@x.com");
    assert_eq!(to, "User@X.com");
    assert_eq!(subject, "Re: Hello");
    let (to, subject) = ch.reply_target_and_subject("other@y.org");
    assert_eq!(to, "other@y.org");
    assert_eq!(subject, "Re: NemesisBot Response");
}

/// allow_from 条目侧在构造期归一小写：身份键已归一（identity_chat_id），
/// BaseChannel 固有 is_allowed 是精确 == 比较——条目不归一会让配置里
/// 原大小写条目错杀归一化后的合法身份（修复前该条目靠原大小写 chat_id
/// 恰好命中，归一化后不处理就是行为回退）。
#[test]
fn test_allow_from_matching_survives_identity_normalization() {
    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "imap_user".to_string(),
        imap_password: "imap_pass".to_string(),
        allow_from: vec!["User@X.com".to_string()],
        ..Default::default()
    };
    let ch = EmailChannel::new(config, test_sender()).unwrap();
    // 归一化身份（入站实际传入形态）命中原大小写条目
    assert!(ch.base.is_allowed("user@x.com"));
    // 非白名单地址仍被拒
    assert!(!ch.base.is_allowed("attacker@evil.com"));
}

// ============================================================
// UID 命令族（BUG-20261117-078）
// ============================================================

/// 收件路径必须用 UID 命令（SEARCH/FETCH/STORE）而非 MSN 序号——并发
/// EXPUNGE（其他会话清理邮箱）会重编号 MSN，批内后续 FETCH/STORE 打错
/// 消息 = 错标已读/内容丢失。UID 稳定唯一（RFC 3501 §2.3.1.2）。
/// 突变验证：摘掉任一 helper 的 "UID " 前缀本测必红。
#[test]
fn test_uid_command_family_prefixes() {
    let uid = "42";
    let env = uid_fetch_envelope_cmd(uid);
    assert!(
        env.starts_with("UID FETCH 42 ")
            && env.contains("ENVELOPE")
            && env.contains("SUBJECT FROM MESSAGE-ID"),
        "envelope 命令必须是 UID FETCH 形态: {env}"
    );
    let body = uid_fetch_body_cmd(uid);
    assert_eq!(body, "UID FETCH 42 (BODY[TEXT])");
    let store = uid_store_seen_cmd(uid);
    assert_eq!(store, "UID STORE 42 +FLAGS (\\Seen)");
}

/// UID SEARCH 与 SEARCH 的 untagged 响应同形态——解析器对 `* SEARCH`
/// 前缀裸数字的提取在两种命令下行为一致（回归锚：解析器不得耦合
/// 命令形态）。
#[test]
fn test_parse_search_results_shape_invariant_across_uid_and_msn() {
    let responses = vec![
        "* SEARCH 2 5 17".to_string(),
        "* 1 EXISTS".to_string(),
        "* SEARCH".to_string(), // 空结果：无未读
    ];
    let parsed = EmailChannel::parse_search_results(&responses);
    assert_eq!(
        parsed,
        vec!["2", "5", "17"],
        "只收 * SEARCH 行的裸数字 token"
    );
}

// ---- BUG-20261118-130：出站非 ASCII 编码面（RFC 2047 Subject / CTE 正文） ----

/// 从消息头区抽取 Subject 值（含 CRLF+WSP 折行连续行——encoded-word 折行
/// 形态的完整值，不得只取首个物理行）。
fn extract_subject_value(msg: &str) -> String {
    let header_region = msg.split("\r\n\r\n").next().expect("header region");
    let mut val = String::new();
    let mut capturing = false;
    for line in header_region.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Subject: ") {
            capturing = true;
            val.push_str(rest);
        } else if capturing && line.starts_with(' ') {
            val.push_str(line.trim_start());
        } else if capturing {
            break;
        }
    }
    val
}

/// encoded-word 序列解码 round-trip：先剥折行/空白（RFC 2047 相邻
/// encoded-word 间的空白解码时丢弃）再逐块独立解码拼接。
fn decode_rfc2047(encoded: &str) -> String {
    let joined = encoded.replace("\r\n ", "").replace(' ', "");
    let mut decoded = Vec::new();
    for chunk in joined.split("=?UTF-8?B?") {
        if let Some(b64) = chunk.strip_suffix("?=") {
            decoded.extend(
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                    .expect("each encoded-word chunk must decode independently"),
            );
        }
    }
    String::from_utf8(decoded).expect("decoded subject must be utf-8")
}

/// 非 ASCII Subject 必须走 RFC 2047 B-encoding：裸 8-bit 头违反 RFC 5322
/// 头 ASCII 域（未协商 SMTPUTF8 的会话严格 MTA 可拒收；收件端 MUA 按本地
/// 编码猜测乱码）。断言：编码形态在场、头区恒纯 ASCII、解码 round-trip
/// 等于原文；ASCII 正文不受影响（无 CTE 头）。
#[test]
fn test_build_smtp_message_rfc2047_encodes_non_ascii_subject() {
    let subject = "Re: 项目周报与评审纪要";
    let msg = EmailChannel::build_smtp_message(
        "bot@example.com",
        "user@example.com",
        subject,
        "ascii body",
    );
    assert!(
        msg.contains("Subject: =?UTF-8?B?"),
        "非 ASCII Subject 必须编码为 encoded-word: {msg}"
    );
    let header_end = msg.find("\r\n\r\n").expect("header/body boundary");
    assert!(
        msg[..header_end].is_ascii(),
        "header region must be pure ASCII: {msg:?}"
    );
    assert_eq!(
        decode_rfc2047(&extract_subject_value(&msg)),
        subject,
        "decoded subject must round-trip"
    );
    // ASCII 正文零漂移：无 CTE 头、正文原样。
    assert!(!msg.contains("Content-Transfer-Encoding"));
    assert!(msg.contains("\r\n\r\nascii body"));
}

/// 超长非 ASCII Subject 折行有界：每块载荷 ≤48 base64 字符（整个
/// encoded-word 60 ≤ RFC 2047 的 75 字符 SHOULD）、4 对齐独立可解；
/// 全序列拼接解码 = 原文。
#[test]
fn test_rfc2047_long_subject_chunked_bounded() {
    let subject = "长".repeat(200); // 200 CJK → 600 UTF-8 bytes → 800 b64 chars
    let enc = super::encode_rfc2047_subject(&subject);
    for word in enc.split("\r\n ") {
        assert!(
            word.starts_with("=?UTF-8?B?") && word.ends_with("?="),
            "fold unit must be a complete encoded-word: {word}"
        );
        let payload = &word[10..word.len() - 2];
        assert!(
            payload.len() <= 48 && payload.len() % 4 == 0,
            "encoded-word payload must be 4-aligned and ≤48: {}",
            payload.len()
        );
        assert!(
            !base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
                .expect("chunk decodes independently")
                .is_empty()
        );
    }
    assert_eq!(decode_rfc2047(&enc), subject);
}

/// 非 ASCII 正文必须带显式 CTE: base64（RFC 2045 缺省 CTE=7bit 与实际
/// 8-bit 不符——严格 MTA 拒信或 8→7bit 转换损坏）。CTE 头恰在 Content-Type
/// 与空行之间；载荷 76 字符折行、解码 round-trip 等于原文。
#[test]
fn test_build_smtp_message_base64_encodes_non_ascii_body() {
    let body = "回复正文第一行\r\n第二行包含中文与 emoji 🎉";
    let msg =
        EmailChannel::build_smtp_message("bot@example.com", "user@example.com", "Re: Hello", body);
    assert!(
        msg.contains(
            "Content-Type: text/plain; charset=UTF-8\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n"
        ),
        "CTE header must sit between Content-Type and the empty line: {msg:?}"
    );
    let body_region = msg.split("\r\n\r\n").nth(1).expect("body region");
    assert!(
        body_region.is_ascii(),
        "base64 body region must be pure ASCII: {body_region:?}"
    );
    for line in body_region.lines() {
        assert!(
            !line.is_empty() && line.len() <= 76,
            "base64 line must be 1..=76 chars, got {}",
            line.len()
        );
    }
    let joined: String = body_region.lines().collect();
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, joined)
        .expect("wrapped base64 must decode");
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        body,
        "decoded body must round-trip (dot-stuffing input unchanged: base64 alphabet has no '.')"
    );
}

/// 注入面与编码面叠加：CR/LF 先剥后编码——「中文\r\nBcc: victim」不得
/// 以任何形态产生注入头（编码输入恒为拍平单行）；解码值 = 拍平后的单行。
#[test]
fn test_rfc2047_subject_flattens_crlf_before_encoding() {
    let msg = EmailChannel::build_smtp_message(
        "bot@example.com",
        "user@example.com",
        "中文\r\nBcc: victim@example.com",
        "b",
    );
    assert!(
        !msg.contains("Bcc: victim@example.com\r\n"),
        "injected header line must not exist: {msg:?}"
    );
    assert_eq!(
        decode_rfc2047(&extract_subject_value(&msg)),
        "中文Bcc: victim@example.com",
        "CRLF must be flattened before encoding"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261120-023：STORE \Seen 发布门禁（吞错 STORE + 照常发布 = 重复风暴）
// ---------------------------------------------------------------------------

/// 多轮询假 IMAP 服务器骨架（accept 循环——每轮 poll 一条新连接）。
///
/// 共享态：`store_no_first`（第一次 STORE 返回 NO，其后 OK 并置 seen 标记）
/// 与 `seen`（置位后 SEARCH 返回空——模拟服务器侧 \Seen 生效）。
/// `store_always_no=true` 时 STORE 恒 NO、seen 永不置位（READ-ONLY 邮箱形态）。
/// 邮件**无 Message-ID 行**（进程内 seen_uids 无法兜底的形态——D1 主证）。
async fn spawn_fake_imap_store_gate(always_no: bool) -> std::net::SocketAddr {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let seen = std::sync::Arc::new(AtomicBool::new(false));
    let store_count = std::sync::Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen_srv = seen.clone();
    let count_srv = store_count.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let (rd, mut wr) = stream.into_split();
            let mut rd = BufReader::new(rd);
            let mut line = String::new();
            if wr.write_all(b"* OK IMAP4rev1 ready\r\n").await.is_err() {
                return;
            }
            loop {
                line.clear();
                if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                let resp = if line.contains("LOGIN") {
                    "NB001 OK LOGIN\r\n".to_string()
                } else if line.contains("SELECT") {
                    "NB002 OK SELECT\r\n".to_string()
                } else if line.contains("SEARCH") {
                    if seen_srv.load(Ordering::SeqCst) {
                        "* SEARCH\r\nNB003 OK SEARCH\r\n".to_string()
                    } else {
                        "* SEARCH 1\r\nNB003 OK SEARCH\r\n".to_string()
                    }
                } else if line.contains("BODY[HEADER") {
                    // 无 Message-ID 行的 header literal（{55} = 实际字节数）：
                    // message_id 解析为空串 → seen_uids 不可兜底
                    "* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {55}\r\n\
                     From: Alice <alice@example.com>\r\n\
                     Subject: Hello there\r\n\
                     )\r\n\
                     NB004 OK FETCH\r\n"
                        .to_string()
                } else if line.contains("BODY[TEXT]") {
                    "* 1 FETCH (BODY[TEXT] {10}\r\nHello body\r\n)\r\nNB005 OK FETCH\r\n"
                        .to_string()
                } else if line.contains("STORE") {
                    let n = count_srv.fetch_add(1, Ordering::SeqCst);
                    if always_no || n == 0 {
                        "NB006 NO STORE failed\r\n".to_string()
                    } else {
                        seen_srv.store(true, Ordering::SeqCst);
                        "NB006 OK STORE\r\n".to_string()
                    }
                } else if line.contains("LOGOUT") {
                    "NB007 OK LOGOUT\r\n".to_string()
                } else {
                    "NB999 OK\r\n".to_string()
                };
                if wr.write_all(resp.as_bytes()).await.is_err() {
                    break;
                }
            }
        }
    });
    addr
}

fn store_gate_config(addr: std::net::SocketAddr) -> EmailConfig {
    EmailConfig {
        imap_host: addr.ip().to_string(),
        imap_port: addr.port(),
        smtp_host: "smtp.example.com".to_string(),
        imap_username: "user".to_string(),
        imap_password: "pass".to_string(),
        poll_interval: 1, // 每秒一轮——观察重复投递/恰好一次语义
        ..Default::default()
    }
}

/// D1 主证（修复前红）：STORE 恒 NO（READ-ONLY 邮箱形态）+ 邮件无
/// Message-ID（seen_uids 无法兜底）→ 修复前吞错 STORE 照常发布，邮件永远
/// UNSEEN = 每轮 poll 重复发布（默认 30s 一条的无限风暴）；修复后 STORE
/// 失败不得发布——窗口内零发布。
#[tokio::test]
async fn test_poll_loop_store_failure_never_publishes() {
    let addr = spawn_fake_imap_store_gate(true).await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let ch = EmailChannel::new(store_gate_config(addr), tx).unwrap();
    ch.start().await.unwrap();

    let leaked = tokio::time::timeout(std::time::Duration::from_millis(3500), rx.recv()).await;
    assert!(
        leaked.is_err(),
        "STORE failure must defer publish (no duplicate storm), got: {leaked:?}"
    );

    ch.stop().await.unwrap();
}

/// D1 恢复面（修复前红）：第一次 STORE NO（瞬时失败）、其后 OK → 修复前
/// 两轮各发布一次（吞错 + 空Message-ID 无进程内去重 = 恰好重复）；修复后
/// 第一轮不发布、第二轮 STORE 成功后发布 = 窗口内**恰好一条**。
#[tokio::test]
async fn test_poll_loop_store_transient_failure_exactly_once() {
    let addr = spawn_fake_imap_store_gate(false).await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let ch = EmailChannel::new(store_gate_config(addr), tx).unwrap();
    ch.start().await.unwrap();

    // 窗口 3.5s ≥ 3 轮 poll（1s 间隔）：第 1 轮 STORE NO 不发布、第 2 轮
    // STORE OK 发布、第 3 轮 SEARCH 空。恰好 1 条。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(3500);
    let mut count = 0usize;
    let mut dupes = Vec::new();
    while std::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remain, rx.recv()).await {
            Ok(Ok(msg)) => {
                count += 1;
                dupes.push(msg.content);
            }
            _ => break,
        }
    }
    assert_eq!(
        count, 1,
        "transient STORE failure must yield exactly-once delivery, got {count}: {dupes:?}"
    );

    ch.stop().await.unwrap();
}

/// BUG-20261125-019 回归锁：start/stop 必须维护 base running 旗标 + 幂等双启
/// 守卫（BUG-20261121-050 同构家族横向扫漏网补齐——manager 的 start_all 守卫 /
/// any_channel_running / status 上报三消费点都读 trait is_running）。
/// 服务端点指 loopback 死端口（后台 receive loop 连接失败零外网，start 本体
/// 不阻塞）；二次 start 走幂等守卫诚实跳过（不 spawn 第二个 receive loop）。
#[tokio::test]
async fn b019_start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let config = EmailConfig {
        imap_host: "127.0.0.1".to_string(),
        smtp_host: "127.0.0.1".to_string(),
        imap_username: "u".to_string(),
        imap_password: "p".to_string(),
        ..Default::default()
    };
    let (tx, _) = tokio::sync::broadcast::channel(16);
    let ch = EmailChannel::new(config, tx).unwrap();
    assert!(!ch.base.is_running());

    ch.start().await.unwrap();
    assert!(
        ch.base.is_running(),
        "start 后 trait is_running 必须为 true（BUG-20261125-019）"
    );
    ch.start().await.unwrap();
    assert!(ch.base.is_running());

    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 后 trait is_running 必须为 false"
    );
}

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

// ---- F5（2026-09-22 审计修复）：SMTP/IMAP IO 超时 ----------------------------

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

// BUG-20261029-142：start() 把 sender_map 交给 IMAP 轮询任务的必须是共享
// 克隆——裸 DashMap::clone() 是逐 shard 深拷贝，任务侧写入的回复线索
// （chat_id -> 发件人/主题）落私有副本，send() 的读路径（self.sender_map
// .get）读原表恒 miss，邮件回复线程化静默失效（Re: 主题恒落默认值）。
// 本测试复刻任务侧捕获形态（与字段同源 .clone()）写入后，断言结构体侧
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

// ---- BUG-20261031-244：收件黑洞（IMAP 轮询到标已读为止，从不发布总线） ----

/// 起一个只讲最小 IMAP 方言的假服务器（单连接）：greeting → LOGIN/SELECT OK →
/// SEARCH 返回 1 封 → FETCH 头（Alice / Hello there / m1）→ FETCH 正文
/// （"Hello body"）→ STORE/LOGOUT OK。命令匹配按内容不按 tag，tag 响应号
/// 与客户端 next_tag 顺序（NB001..NB007）对齐。
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
                "* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {68}\r\n\
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
        poll_interval: 3600, // 首 tick 立即触发，测试期内不再第二轮询
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

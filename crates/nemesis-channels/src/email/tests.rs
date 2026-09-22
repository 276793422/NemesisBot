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
    let ch = EmailChannel::new(config).unwrap();

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

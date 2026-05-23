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

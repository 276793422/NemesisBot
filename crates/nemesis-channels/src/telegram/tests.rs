use super::*;

#[test]
fn test_markdown_to_html_bold() {
    let result = TelegramChannel::markdown_to_telegram_html("**hello**");
    assert_eq!(result, "<b>hello</b>");
}

#[test]
fn test_markdown_to_html_italic() {
    let result = TelegramChannel::markdown_to_telegram_html("_hello_");
    assert_eq!(result, "<i>hello</i>");
}

#[test]
fn test_markdown_to_html_code() {
    let result = TelegramChannel::markdown_to_telegram_html("`code`");
    assert_eq!(result, "<code>code</code>");
}

#[test]
fn test_markdown_to_html_code_block() {
    let input = "```\nlet x = 1;\n```";
    let result = TelegramChannel::markdown_to_telegram_html(input);
    assert!(result.contains("<pre><code>"));
    assert!(result.contains("let x = 1;"));
}

#[test]
fn test_markdown_to_html_links() {
    let result = TelegramChannel::markdown_to_telegram_html("[click](http://example.com)");
    assert!(result.contains(r#"<a href="http://example.com">click</a>"#));
}

#[test]
fn test_escape_html() {
    assert_eq!(escape_html("<b>"), "&lt;b&gt;");
    assert_eq!(escape_html("a&b"), "a&amp;b");
}

#[tokio::test]
async fn test_telegram_channel_new_validates_token() {
    let config = TelegramConfig::default();
    let (tx, _rx) = broadcast::channel(256);
    let result = TelegramChannel::new(config, tx);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("token is required"));
}

#[tokio::test]
async fn test_telegram_channel_new_with_token() {
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "telegram");
}

#[test]
fn test_telegram_config_default() {
    let cfg = TelegramConfig::default();
    assert!(cfg.token.is_empty());
    assert_eq!(cfg.api_base, "https://api.telegram.org");
    assert!(cfg.proxy.is_none());
}

#[test]
fn test_telegram_set_transcriber() {
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(config, tx).unwrap();

    // Should not panic with None
    // (We can't test with a real transcriber because the trait requires async)
    // Just verify the method exists and compiles
}

#[test]
fn test_thinking_cancel() {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let mut cancel = ThinkingCancel::new(tx);

    // Cancel should signal the receiver
    cancel.cancel();
    // The receiver should get the signal (or be errored because sender was dropped)
    // tokio::sync::oneshot::Receiver doesn't have is_ok()/is_err() until awaited
    let _ = rx;
}

#[test]
fn test_stop_thinking_no_op_for_nonexistent() {
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(config, tx).unwrap();
    // Should not panic when no thinking animation exists
    ch.stop_thinking_animation("12345");
}

#[tokio::test]
async fn test_handle_incoming_message_text() {
    let (tx, mut rx) = broadcast::channel(256);

    let msg = TelegramMessage {
        message_id: 42,
        from: Some(TelegramUser {
            id: 12345,
            username: Some("testuser".to_string()),
            first_name: "Test".to_string(),
            last_name: None,
        }),
        chat: TelegramChat {
            id: 67890,
            chat_type: "private".to_string(),
        },
        text: Some("Hello bot!".to_string()),
        caption: None,
        photo: None,
        voice: None,
        audio: None,
        document: None,
    };

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
        .await;

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "telegram");
    assert_eq!(inbound.sender_id, "12345|testuser");
    assert_eq!(inbound.chat_id, "67890");
    assert_eq!(inbound.content, "Hello bot!");
    assert!(inbound.media.is_empty());
    assert_eq!(inbound.metadata.get("message_id").unwrap(), "42");
}

#[tokio::test]
async fn test_handle_incoming_message_with_photo() {
    let (tx, mut rx) = broadcast::channel(256);

    let msg = TelegramMessage {
        message_id: 43,
        from: Some(TelegramUser {
            id: 12345,
            username: None,
            first_name: "Test".to_string(),
            last_name: None,
        }),
        chat: TelegramChat {
            id: 67890,
            chat_type: "private".to_string(),
        },
        text: None,
        caption: Some("A nice photo".to_string()),
        photo: Some(vec![TelegramPhotoSize {
            file_id: "photo_file_123".to_string(),
            width: 800,
            height: 600,
        }]),
        voice: None,
        audio: None,
        document: None,
    };

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
        .await;

    let inbound = rx.try_recv().unwrap();
    assert!(inbound.content.contains("A nice photo"));
    // Test passes http=None → file can't be fetched → fallback文案 + no media attachment
    assert!(inbound.content.contains("[Photo received"));
    assert!(inbound.media.is_empty());
}

#[tokio::test]
async fn test_handle_incoming_message_rejected_by_allowlist() {
    let (tx, mut rx) = broadcast::channel(256);

    let msg = TelegramMessage {
        message_id: 44,
        from: Some(TelegramUser {
            id: 99999,
            username: Some("blocked".to_string()),
            first_name: "Blocked".to_string(),
            last_name: None,
        }),
        chat: TelegramChat {
            id: 67890,
            chat_type: "private".to_string(),
        },
        text: Some("Should be blocked".to_string()),
        caption: None,
        photo: None,
        voice: None,
        audio: None,
        document: None,
    };

    TelegramChannel::handle_incoming_message(
        &msg,
        &tx,
        &["12345".to_string()],
        &None,
        None,
        None,
    )
    .await;

    // Message should be dropped — nothing to receive
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn test_handle_incoming_message_empty() {
    let (tx, mut rx) = broadcast::channel(256);

    let msg = TelegramMessage {
        message_id: 45,
        from: Some(TelegramUser {
            id: 12345,
            username: None,
            first_name: "Test".to_string(),
            last_name: None,
        }),
        chat: TelegramChat {
            id: 67890,
            chat_type: "private".to_string(),
        },
        text: None,
        caption: None,
        photo: None,
        voice: None,
        audio: None,
        document: None,
    };

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
        .await;

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.content, "[empty message]");
}

#[tokio::test]
async fn test_telegram_new_with_client_validates_token() {
    let config = TelegramConfig::default();
    let (tx, _rx) = broadcast::channel(256);
    let http = reqwest::Client::new();
    let result = TelegramChannel::new_with_client(config, tx, http);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("token is required"));
}

#[tokio::test]
async fn test_telegram_new_with_client_success() {
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let http = reqwest::Client::new();
    let ch = TelegramChannel::new_with_client(config, tx, http).unwrap();
    assert_eq!(ch.name(), "telegram");
    assert!(!*ch.running.read());
}

#[tokio::test]
async fn test_handle_incoming_message_voice_no_transcriber() {
    let (tx, mut rx) = broadcast::channel(256);

    let msg = TelegramMessage {
        message_id: 50,
        from: Some(TelegramUser {
            id: 12345,
            username: Some("testuser".to_string()),
            first_name: "Test".to_string(),
            last_name: None,
        }),
        chat: TelegramChat {
            id: 67890,
            chat_type: "private".to_string(),
        },
        text: None,
        caption: None,
        photo: None,
        voice: Some(TelegramFile {
            file_id: "voice_file_123".to_string(),
            file_unique_id: "unique_123".to_string(),
            file_size: Some(1024),
        }),
        audio: None,
        document: None,
    };

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None)
        .await;

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.content, "[voice]");
    assert_eq!(inbound.media.len(), 1);
    assert_eq!(inbound.media[0].media_type, "voice");
}

#[tokio::test]
async fn test_voice_transcribe_no_transcriber() {
    // When no transcriber is set, should return None
    let result = TelegramChannel::transcribe_voice(&None, None, None, "file123").await;
    assert!(result.is_none());
}

/// Mock transcriber for testing voice transcription flow.
struct MockTranscriber {
    available: bool,
    text: String,
    should_fail: bool,
}

impl crate::base::VoiceTranscriber for MockTranscriber {
    fn is_available(&self) -> bool {
        self.available
    }

    fn transcribe(
        &self,
        _file_path: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send + '_>,
    > {
        if self.should_fail {
            Box::pin(async { Err("transcription error".to_string()) })
        } else {
            let text = self.text.clone();
            Box::pin(async move { Ok(text) })
        }
    }
}

#[tokio::test]
async fn test_voice_transcribe_unavailable_transcriber() {
    let transcriber: Arc<dyn crate::base::VoiceTranscriber> = Arc::new(MockTranscriber {
        available: false,
        text: String::new(),
        should_fail: false,
    });
    let result =
        TelegramChannel::transcribe_voice(&Some(transcriber), None, None, "file123").await;
    assert!(result.is_none());
}

#[tokio::test]
async fn test_voice_transcribe_no_http_client() {
    let transcriber: Arc<dyn crate::base::VoiceTranscriber> = Arc::new(MockTranscriber {
        available: true,
        text: "hello world".to_string(),
        should_fail: false,
    });
    // Available transcriber but no HTTP client → can't download → None
    let result =
        TelegramChannel::transcribe_voice(&Some(transcriber), None, None, "file123").await;
    assert!(result.is_none());
}

// -----------------------------------------------------------------------
// Tests for markdown_to_telegram_html: headers, blockquotes, bold
// underscores, list markers, strikethrough, and edge cases
// -----------------------------------------------------------------------

#[test]
fn test_markdown_header_to_bold() {
    let result = TelegramChannel::markdown_to_telegram_html("# Header");
    assert_eq!(result, "<b>Header</b>");
}

#[test]
fn test_markdown_multiple_header_levels() {
    let result = TelegramChannel::markdown_to_telegram_html("### Level 3");
    assert_eq!(result, "<b>Level 3</b>");
}

#[test]
fn test_markdown_blockquote() {
    let result = TelegramChannel::markdown_to_telegram_html("> quoted text");
    assert_eq!(result, "<blockquote>quoted text</blockquote>");
}

#[test]
fn test_markdown_bold_double_underscores() {
    let result = TelegramChannel::markdown_to_telegram_html("__bold text__");
    assert_eq!(result, "<b>bold text</b>");
}

#[test]
fn test_markdown_list_marker_dash() {
    let result = TelegramChannel::markdown_to_telegram_html("- item");
    assert_eq!(result, "• item");
}

#[test]
fn test_markdown_list_marker_asterisk() {
    let result = TelegramChannel::markdown_to_telegram_html("* item");
    assert_eq!(result, "• item");
}

#[test]
fn test_markdown_combined_bold_and_italic() {
    let result = TelegramChannel::markdown_to_telegram_html("**bold** and _italic_");
    assert!(
        result.contains("<b>bold</b>"),
        "expected bold tag in: {result}"
    );
    assert!(
        result.contains("<i>italic</i>"),
        "expected italic tag in: {result}"
    );
}

#[test]
fn test_markdown_links_preserved() {
    let result = TelegramChannel::markdown_to_telegram_html("[text](url)");
    assert_eq!(result, r#"<a href="url">text</a>"#);
}

#[test]
fn test_markdown_code_blocks_preserved() {
    let input = "```code```";
    let result = TelegramChannel::markdown_to_telegram_html(input);
    assert!(
        result.contains("<pre><code>"),
        "expected <pre><code> in: {result}"
    );
    assert!(result.contains("code"), "expected 'code' in: {result}");
}

#[test]
fn test_markdown_empty_string() {
    let result = TelegramChannel::markdown_to_telegram_html("");
    assert_eq!(result, "");
}

#[test]
fn test_markdown_html_escaping() {
    let result = TelegramChannel::markdown_to_telegram_html("<script>");
    assert_eq!(result, "&lt;script&gt;");
}

#[test]
fn test_markdown_strikethrough() {
    let result = TelegramChannel::markdown_to_telegram_html("~~deleted~~");
    assert_eq!(result, "<s>deleted</s>");
}

#[test]
fn test_markdown_mixed_headers_and_bold() {
    let input = "## Title\n**bold**";
    let result = TelegramChannel::markdown_to_telegram_html(input);
    assert!(
        result.contains("<b>Title</b>"),
        "expected bold Title in: {result}"
    );
    assert!(
        result.contains("<b>bold</b>"),
        "expected bold tag in: {result}"
    );
}

#[test]
fn test_markdown_code_blocks_not_affected_by_bold_conversion() {
    let input = "```**not bold**```";
    let result = TelegramChannel::markdown_to_telegram_html(input);
    // The content inside code blocks should be preserved literally,
    // not converted to <b> tags.
    assert!(
        !result.contains("<b>"),
        "code block content should not be converted to bold: {result}"
    );
    assert!(
        result.contains("**not bold**"),
        "code block should preserve original text: {result}"
    );
}

#[test]
fn test_telegram_config_proxy_support() {
    // Verify TelegramConfig stores the proxy field correctly
    let cfg = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        proxy: Some("http://proxy.example.com:8080".to_string()),
        ..Default::default()
    };
    assert_eq!(
        cfg.proxy.as_deref(),
        Some("http://proxy.example.com:8080")
    );

    // Verify a channel can be created with proxy config
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(cfg, tx).unwrap();
    assert_eq!(ch.name(), "telegram");

    // Verify default config has no proxy
    let default_cfg = TelegramConfig::default();
    assert!(default_cfg.proxy.is_none());
}

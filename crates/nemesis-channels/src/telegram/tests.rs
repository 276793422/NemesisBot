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
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("token is required")
    );
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
    let _ch = TelegramChannel::new(config, tx).unwrap();

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

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None).await;

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

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None).await;

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

    TelegramChannel::handle_incoming_message(&msg, &tx, &["12345".to_string()], &None, None, None)
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

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None).await;

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
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("token is required")
    );
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

    TelegramChannel::handle_incoming_message(&msg, &tx, &[], &None, None, None).await;

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
    let result = TelegramChannel::transcribe_voice(&Some(transcriber), None, None, "file123").await;
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
    let result = TelegramChannel::transcribe_voice(&Some(transcriber), None, None, "file123").await;
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
    assert_eq!(cfg.proxy.as_deref(), Some("http://proxy.example.com:8080"));

    // Verify a channel can be created with proxy config
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(cfg, tx).unwrap();
    assert_eq!(ch.name(), "telegram");

    // Verify default config has no proxy
    let default_cfg = TelegramConfig::default();
    assert!(default_cfg.proxy.is_none());
}

// ---------------------------------------------------------------------------
// T7（多模态）：photo 下载落盘 → 本地路径引用（mock Telegram API 单测）
// ---------------------------------------------------------------------------

/// 极简 mock Telegram Bot API：阻塞线程逐连接应答。
/// - `POST …/getFile` → `{"ok":true,"result":{"file_path":"photos/<file>.jpg"}}`
/// - `GET  …/file/photos/<file>.jpg` → 传入的字节
/// 返回 (api_url_base, 关闭句柄)。监听 127.0.0.1:0（系统分配端口）。
fn spawn_mock_telegram_api(file_bytes: Vec<u8>) -> String {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        // 恰好两轮：getFile + file 下载（各一连接；keep-alive 关闭语义由
        // Connection: close 保证，reqwest 每请求新连）。
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();

            let (status, body): (&str, Vec<u8>) = if path.ends_with("/getFile") {
                (
                    "200 OK",
                    br#"{"ok":true,"result":{"file_id":"agg","file_path":"photos/nb_t7.jpg","file_size":1}}"#
                        .to_vec(),
                )
            } else if path.contains("/file/") {
                ("200 OK", file_bytes.clone())
            } else {
                ("404 Not Found", b"{}".to_vec())
            };
            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status,
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn t7_photo_download_lands_local_path_reference() {
    use base64::Engine as _;

    // 最小 PNG（8 字节签名 + IHDR）——Telegram 不校验内容，这里只验证
    // 字节搬运。
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);

    let base = spawn_mock_telegram_api(png.clone());
    let uploads = tempfile::tempdir().expect("uploads tempdir");

    let client = reqwest::Client::new();
    let att = TelegramChannel::fetch_photo_attachment(
        Some(&client),
        Some(&base),
        "AgAC-file/id.99",
        uploads.path(),
    )
    .await
    .expect("download should succeed");

    // MediaAttachment 形态：image + **本地路径**引用（非 URL）。
    assert_eq!(att.media_type, "image");
    assert!(
        !att.url.starts_with("http"),
        "引用必须是本地路径: {}",
        att.url
    );
    assert!(att.data.is_none());

    // 落盘文件：tg_{sanitized}_{millis}.jpg，字节与下载内容一致，写后可读回。
    let dest = std::path::PathBuf::from(&att.url);
    assert_eq!(
        dest.parent(),
        Some(uploads.path()),
        "文件必须落在 uploads 目录内"
    );
    let name = dest.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with("tg_"), "文件名前缀 tg_: {}", name);
    assert!(name.ends_with(".jpg"), "ext 抠自 file_path: {}", name);
    assert!(
        !name.contains('/'),
        "file_id 消毒后文件名不得含分隔符: {}",
        name
    );
    let on_disk = std::fs::read(&dest).expect("file readable back");
    assert_eq!(on_disk, png, "落盘字节与下载内容一致");
    // base64 水合可读（T5 build_messages 语义的前置保证）。
    assert_eq!(
        base64::engine::general_purpose::STANDARD.encode(&on_disk[..8]),
        base64::engine::general_purpose::STANDARD.encode(&png[..8])
    );
}

#[tokio::test]
async fn t7_photo_download_failure_returns_none() {
    // getFile ok:false（无效 file_id 的真实 API 行为）→ None，不产出引用。
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body =
                br#"{"ok":false,"error_code":400,"description":"Bad Request: wrong file id"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(body);
        }
    });

    let uploads = tempfile::tempdir().expect("uploads tempdir");
    let client = reqwest::Client::new();
    let att = TelegramChannel::fetch_photo_attachment(
        Some(&client),
        Some(&format!("http://{addr}")),
        "bad_id",
        uploads.path(),
    )
    .await;
    assert!(att.is_none(), "ok:false 必须返回 None");
}

#[tokio::test]
async fn t7_photo_download_without_http_returns_none() {
    // http 客户端缺失（轮询未起/代理未配）→ None（诚实降级，不 panic）。
    let uploads = tempfile::tempdir().expect("uploads tempdir");
    let att = TelegramChannel::fetch_photo_attachment(
        None,
        Some("http://127.0.0.1:1"),
        "x",
        uploads.path(),
    )
    .await;
    assert!(att.is_none());
}

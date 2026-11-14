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
    // tokio::sync:oneshot::Receiver doesn't have is_ok()/is_err() until awaited
    drop(rx);
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

/// 奇数个孤立标记（BUG-20261111-063）：<i> 收尾必须补闭合——Telegram
/// HTML parse mode 拒收不闭合标签，sendMessage 400 → 整块回退纯文本，
/// 同块内全部格式丢失。
#[test]
fn test_markdown_odd_italic_marker_closes_dangling_tag() {
    // 单个孤立 * → <i> 收尾闭合，输出恒平衡。
    let result = TelegramChannel::markdown_to_telegram_html("3 * 4 = 12");
    assert_eq!(result, "3 <i> 4 = 12</i>");

    // 三个孤立标记：一开一合 + 悬空闭合。
    let result = TelegramChannel::markdown_to_telegram_html("a * b * c * d");
    assert_eq!(result, "a <i> b </i> c <i> d</i>");

    // 孤立 _ 同处理。
    let result = TelegramChannel::markdown_to_telegram_html("var _ foo");
    assert_eq!(result, "var <i> foo</i>");
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
// （多模态）：photo 下载落盘 → 本地路径引用（mock Telegram API 单测）
// ---------------------------------------------------------------------------

/// 极简 mock Telegram Bot API：阻塞线程逐连接应答。
/// - `POST …/getFile` → `{"ok":true,"result":{"file_path":"photos/<file>.jpg"}}`
/// - `GET …/file/bot{token}/photos/<file>.jpg` → 传入的字节
///
/// 下载路径按**官方命名空间**严格匹配（`/file/bot{token}/` 前缀）——
/// BUG-20261109-006 回归锁：此前代码拼成 `{bot{token}}/file/`（命名空间
/// 交换），而本 mock 旧版用宽松 `contains("/file/")` 匹配把断链放行。
/// 返回 (api_base, 关闭句柄)。监听 127.0.0.1:0（系统分配端口）。
fn spawn_mock_telegram_api(file_bytes: Vec<u8>, token: &str) -> String {
    use std::io::{Read, Write};

    let download_prefix = format!("/file/bot{token}/");
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
            } else if path.starts_with(&download_prefix) {
                // 官方文件命名空间契约锁：/file/bot{token}/ 前缀必须精确在场
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

    let base = spawn_mock_telegram_api(png.clone(), "123456:nb-t7-token");
    let api = TelegramApi::new(&base, "123456:nb-t7-token");
    let uploads = tempfile::tempdir().expect("uploads tempdir");

    let client = reqwest::Client::new();
    let att = TelegramChannel::fetch_file_attachment(
        Some(&client),
        Some(&api),
        "AgAC-file/id.99",
        "image",
        "jpg",
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
    // base64 水合可读（build_messages 语义的前置保证）。
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
    let api = TelegramApi::new(&format!("http://{addr}"), "t");
    let att = TelegramChannel::fetch_file_attachment(
        Some(&client),
        Some(&api),
        "bad_id",
        "image",
        "jpg",
        uploads.path(),
    )
    .await;
    assert!(att.is_none(), "ok:false 必须返回 None");
}

#[tokio::test]
async fn t7_photo_download_without_http_returns_none() {
    // http 客户端缺失（轮询未起/代理未配）→ None（诚实降级，不 panic）。
    let uploads = tempfile::tempdir().expect("uploads tempdir");
    let api = TelegramApi::new("http://127.0.0.1:1", "t");
    let att = TelegramChannel::fetch_file_attachment(
        None,
        Some(&api),
        "x",
        "image",
        "jpg",
        uploads.path(),
    )
    .await;
    assert!(att.is_none());
}

// ---------------------------------------------------------------------------
// BUG-20261109-006：URL 构造契约 + token 脱敏
// ---------------------------------------------------------------------------

/// 文件下载 URL 必须是官方命名空间 `{base}/file/bot{token}/{path}`——
/// 此前三处拼成 `{base}/bot{token}/file/{path}`（命名空间交换），
/// 照片/语音/音频/文档文件访问全部 404，被降级文案掩盖。
#[test]
fn telegram_api_file_url_uses_official_file_namespace() {
    let api = TelegramApi::new("https://api.telegram.org", "123456:ABC-DEF");
    assert_eq!(
        api.file_url("photos/nb.jpg"),
        "https://api.telegram.org/file/bot123456:ABC-DEF/photos/nb.jpg",
        "文件下载必须走 /file/bot{{token}}/ 命名空间"
    );
    assert_eq!(
        api.method_url("getFile"),
        "https://api.telegram.org/bot123456:ABC-DEF/getFile",
        "方法调用走 /bot{{token}}/ 命名空间"
    );
}

/// reqwest 错误 Display 内嵌完整 URL（token 在路径里）——所有格式化
/// 错误的位点必须过 redact_token；空 token（未配置）原样透传。
#[test]
fn redact_token_masks_bot_token_in_error_strings() {
    let err =
        "error sending request for url (https://api.telegram.org/bot123456:SECRET-TOK/sendMessage)";
    let redacted = redact_token(err, "123456:SECRET-TOK");
    assert!(
        !redacted.contains("SECRET-TOK"),
        "token 必须被脱敏: {redacted}"
    );
    assert!(
        redacted.contains("/bot<redacted>/sendMessage"),
        "形态可读（保留 URL 结构）: {redacted}"
    );
    // 空 token = 未配置，原样透传（不做无意义替换）
    assert_eq!(redact_token("plain error", ""), "plain error");
}

// ---------------------------------------------------------------------------
// download_voice_file 状态码前置闸——404/500 错误体不得
// 被当语音 OGG 落盘送转写。本地 loopback TCP mock（无新 dev-dep）。
// ---------------------------------------------------------------------------

/// Bind 127.0.0.1:0，对每个进来的请求回同一份固定 HTTP 响应，直到 listener
/// 被 drop。
fn spawn_status_mock(status_line: &'static str, body: &'static str) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

#[tokio::test]
async fn t7_voice_download_rejects_http_error_pages() {
    let port = spawn_status_mock("HTTP/1.1 404 Not Found", "<html>nope</html>");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("voice.ogg");
    let api = TelegramApi::new("http://127.0.0.1", "t");
    let err = TelegramChannel::download_voice_file(
        &http,
        &format!("http://127.0.0.1:{port}/file/bot/x.ogg"),
        &target,
        &api,
    )
    .await
    .unwrap_err();
    assert!(err.contains("404"), "error must carry HTTP status: {err}");
    // 错误页不得落盘（旧行为：404 HTML 被写成 voice.ogg 送转写）。
    assert!(!target.exists(), "error body must not be written to disk");
}

#[tokio::test]
async fn t7_voice_download_writes_success_body() {
    let port = spawn_status_mock("HTTP/1.1 200 OK", "OGGDUMMYBYTES");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("voice.ogg");
    let api = TelegramApi::new("http://127.0.0.1", "t");
    TelegramChannel::download_voice_file(
        &http,
        &format!("http://127.0.0.1:{port}/file/bot/x.ogg"),
        &target,
        &api,
    )
    .await
    .expect("200 must download and write");
    let content = std::fs::read_to_string(&target).unwrap();
    assert_eq!(content, "OGGDUMMYBYTES");
}

#[tokio::test]
async fn t7_voice_download_transport_error_is_redacted() {
    // 传输层错误（连接拒绝）的 Display 带 URL（token 在路径里）——
    // 上抛的 Err 串必须已脱敏（BUG-20261109-006）。
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("voice.ogg");
    let api = TelegramApi::new("http://127.0.0.1", "123456:SECRET-VTOK");
    let err = TelegramChannel::download_voice_file(
        &http,
        &format!("http://127.0.0.1:9/file/bot{}/x.ogg", "123456:SECRET-VTOK"),
        &target,
        &api,
    )
    .await
    .unwrap_err();
    assert!(
        !err.contains("SECRET-VTOK"),
        "传输错误串必须脱敏 token: {err}"
    );
}

// ---------------------------------------------------------------------------
// send() 长消息分块——mock Telegram API 功能实跑
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_splits_long_message_into_multiple_api_calls() {
    use std::io::{Read, Write};

    // mock Telegram Bot API：恰好 3 轮 sendMessage 应答，记录请求体。
    // 7500 'a' 字符按 3500 预算拆 [3500, 3500, 500] = 3 次 API 调用。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().unwrap();
    let bodies: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let bodies_srv = bodies.clone();
    std::thread::spawn(move || {
        for _ in 0..3 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            if path.ends_with("/sendMessage") {
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                bodies_srv.lock().unwrap().push(body);
            }
            let resp_body = br#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"ok"}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp_body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(resp_body);
        }
    });

    let config = TelegramConfig {
        api_base: format!("http://{addr}"),
        token: "test-token".to_string(),
        ..Default::default()
    };
    let (bus_tx, _bus_rx) = tokio::sync::broadcast::channel(16);
    // no_proxy：测试环境可能带系统代理，loopback mock 必须直连。
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let channel = TelegramChannel::new_with_client(config, bus_tx, http).unwrap();
    // send() 的 running 闸直接置位（子模块测试可访问私有字段），绕过 start()
    // 的 getMe/polling 装配——本测例只验证投递分块行为。
    *channel.running.write() = true;

    let msg = OutboundMessage::new("telegram", "123", &"a".repeat(7500));
    Channel::send(&channel, msg)
        .await
        .expect("send must succeed");

    let recorded = bodies.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3, "7500 字符必须拆成 3 次 sendMessage");
    for body in &recorded {
        assert!(
            body.contains(r#""parse_mode":"HTML""#),
            "每块都走 HTML 通道: {}",
            &body[..body.len().min(120)]
        );
        // 提取 "text":"..." 字段（'a' 纯文本无转义）：serde 字段序
        // chat_id→text→parse_mode，text 收尾于 `","parse_mode"`。
        let start = body.find(r#""text":""#).expect("text field") + r#""text":""#.len();
        let end = body[start..]
            .find(r#"","parse_mode""#)
            .expect("text field terminator")
            + start;
        let text = &body[start..end];
        assert!(
            text.chars().count() <= 4096,
            "单块文本必须低于 Telegram 硬限 4096: {}",
            text.chars().count()
        );
    }
    let total: usize = recorded
        .iter()
        .map(|b| {
            let start = b.find(r#""text":""#).unwrap() + r#""text":""#.len();
            let end = b[start..].find(r#"","parse_mode""#).unwrap() + start;
            end - start
        })
        .sum();
    assert_eq!(total, 7500, "分块不得丢字：三块文本总长必须等于原文");
}

#[tokio::test]
async fn send_splits_astral_dense_message_within_utf16_budget() {
    use std::io::{Read, Write};

    // BUG-20261113-166 回归锁（wire 级）：Telegram 长度口径是 UTF-16
    // code units（官方 entities 文档），astral emoji = 2 units。3000 个
    // 🍎 = 标量 3000 < 3500（标量口径会原样单块直发）但 6000 units > 4096
    // 硬限——修复后必须拆 2 块（[3500, 2500] units）且逐块 ≤ 4096 units。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().unwrap();
    let bodies: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let bodies_srv = bodies.clone();
    std::thread::spawn(move || {
        fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
            hay.windows(needle.len()).position(|w| w == needle)
        }
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // 12KB 级请求体可能被 hyper 拆多次 write——读循环直至
            // Content-Length 满足，单次 read 拿不全。字节级解析（头是
            // ASCII；body 长度比字节数，避免 lossy 截断多字节虚增）。
            let mut req: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 65536];
            loop {
                let n = stream.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&tmp[..n]);
                let Some(hdr_end) = find_sub(&req, b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&req[..hdr_end]).to_lowercase();
                let clen = headers
                    .split("content-length:")
                    .nth(1)
                    .and_then(|x| x.split("\r\n").next())
                    .and_then(|x| x.trim().parse::<usize>().ok());
                match clen {
                    Some(c) => {
                        if req.len() - hdr_end - 4 >= c {
                            break;
                        }
                    }
                    None => break,
                }
            }
            let req = String::from_utf8_lossy(&req).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            if path.ends_with("/sendMessage") {
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                bodies_srv.lock().unwrap().push(body);
            }
            let resp_body = br#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"ok"}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp_body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(resp_body);
        }
    });

    let config = TelegramConfig {
        api_base: format!("http://{addr}"),
        token: "test-token".to_string(),
        ..Default::default()
    };
    let (bus_tx, _bus_rx) = tokio::sync::broadcast::channel(16);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let channel = TelegramChannel::new_with_client(config, bus_tx, http).unwrap();
    *channel.running.write() = true;

    let content = "🍎".repeat(3000); // 12000 bytes / 3000 scalars / 6000 units
    let msg = OutboundMessage::new("telegram", "123", &content);
    Channel::send(&channel, msg)
        .await
        .expect("send must succeed");

    let recorded = bodies.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        2,
        "6000 units 必须按 3500 预算拆成 2 次 sendMessage"
    );
    let mut total_units = 0usize;
    for body in &recorded {
        // 🍎 非 HTML 特殊字符，markdown→HTML 转换不改字节；JSON 体不转义
        // 非 ASCII——text 字段即原始 emoji 序列。
        let start = body.find(r#""text":""#).expect("text field") + r#""text":""#.len();
        let end = body[start..]
            .find(r#"","parse_mode""#)
            .expect("text field terminator")
            + start;
        let text = &body[start..end];
        let units: usize = text.chars().map(char::len_utf16).sum();
        assert!(
            units <= 4096,
            "单块 UTF-16 units 必须低于 Telegram 硬限 4096: {units}"
        );
        total_units += units;
    }
    assert_eq!(
        total_units, 6000,
        "分块不得丢字：两块 units 总和必须等于原文"
    );
}

// ---：入站 preview 截断必须字符安全 ---

#[test]
fn bug280_message_preview_multibyte_safe() {
    // 修复前：&content[..50] 字节直切，50 字节落在多字节字符中间 panic
    // （debug 日志开启 + 中文消息即触发）。
    let chinese = "中".repeat(60); // 180 字节，第 50 字节在「中」的中间
    let p = super::message_preview(&chinese); // 修复前这一行直接 panic
    assert_eq!(p.chars().count(), 50);
    assert!(p.starts_with(&"中".repeat(50)));

    // 混合字符（emoji = 4 字节单 char）同样安全。
    let mixed = "👍".repeat(30);
    let p = super::message_preview(&mixed);
    assert_eq!(p.chars().count(), 30);

    // 短消息原样返回。
    assert_eq!(super::message_preview("hi"), "hi");
    assert_eq!(super::message_preview(""), "");
}

// BUG-20261109-012 回归锁：api_base 尾斜杠归一——config 反序列化直通，
// 用户从文档复制带斜杠 base（`https://api.telegram.org/`）会拼出
// `//bot…` 双斜杠路径，上游归一行为不可赌，404 即通道整体断链。
#[test]
fn telegram_api_base_trailing_slash_normalized() {
    let api = TelegramApi::new("https://api.telegram.org/", "123:abc");
    assert!(
        api.url_base
            .starts_with("https://api.telegram.org/bot123:abc"),
        "url_base 不得残留双斜杠: {}",
        api.url_base
    );
    assert!(
        api.file_url_base
            .starts_with("https://api.telegram.org/file/bot123:abc")
    );
    // 无尾斜杠形态幂等（归一前后一致）
    let plain = TelegramApi::new("https://api.telegram.org", "123:abc");
    assert_eq!(api.url_base, plain.url_base);
    assert_eq!(api.file_url_base, plain.file_url_base);
}

#[test]
fn test_allowlist_username_case_insensitive() {
    // BUG-20261109-032①：Telegram 用户名平台侧大小写不敏感（@Alice 与
    // @alice 同一账号）——配置大小写手滑不得误拒合法用户
    assert!(super::telegram_allowlisted(
        &["@Alice".to_string()],
        "123|alice",
        Some("alice")
    ));
    assert!(super::telegram_allowlisted(
        &["alice".to_string()],
        "123|ALICE",
        Some("ALICE")
    ));
    assert!(super::telegram_allowlisted(
        &["ALICE".to_string()],
        "999",
        Some("alice")
    ));
}

#[test]
fn test_allowlist_no_numeric_prefix_collision() {
    // BUG-20261109-032②：裸前缀臂已删——allowed "1234" 不得命中
    // "123456|bob"（Telegram 顺序发号使数字前缀碰撞现实可得，
    // 旧形态是越权放行）
    assert!(!super::telegram_allowlisted(
        &["1234".to_string()],
        "123456|bob",
        Some("bob")
    ));
    assert!(!super::telegram_allowlisted(
        &["1234|alice".to_string()],
        "123456|alice",
        Some("alice")
    ));
}

#[test]
fn test_allowlist_compound_and_bare_id_forms() {
    // 回归护栏：删臂后合法形态全保留——bare id 复合前缀 / 全复合精确 /
    // 复合条目大小写漂移走用户名折叠臂
    assert!(super::telegram_allowlisted(
        &["123".to_string()],
        "123|alice",
        Some("alice")
    ));
    assert!(super::telegram_allowlisted(
        &["123|alice".to_string()],
        "123|alice",
        Some("alice")
    ));
    assert!(super::telegram_allowlisted(
        &["123|Alice".to_string()],
        "123|alice",
        Some("alice")
    ));
    assert!(!super::telegram_allowlisted(
        &["456".to_string()],
        "123|alice",
        Some("alice")
    ));
    assert!(!super::telegram_allowlisted(
        &["@carol".to_string()],
        "123|alice",
        Some("alice")
    ));
}

// ---------------------------------------------------------------------------
// BUG-20261110-037：restart/重复 start 的双 poll 循环竞态回归。
// 三个守卫面：start 幂等守卫 / stop 清旗标 / polling_loop 句柄替换+abort。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_start_guard_skips_when_already_running() {
    // 幂等守卫：base 旗标为 true 时 start() 必须在任何网络调用前短路。
    // 守卫若失效，会继续走 get_me() 对 dummy token 发真请求——离线 / 401
    // 两种环境都返回 Err，测试即红。
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(config, tx).unwrap();
    ch.base.set_running(true);

    assert!(ch.start().await.is_ok());
    assert!(
        ch.poll_handle.lock().is_none(),
        "守卫命中时不得 spawn poll 任务"
    );
}

#[tokio::test]
async fn test_stop_clears_running_flags() {
    // stop 必须同时清本地旗标与 base 旗标——后者是 manager
    // skip-if-running 守卫的判定面，漏清会让重启后的 start_all 永久跳过本通道。
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(config, tx).unwrap();
    *ch.running.write() = true;
    ch.base.set_running(true);

    ch.stop().await.unwrap();
    assert!(!*ch.running.read(), "本地旗标必须被 stop 清除");
    assert!(
        !ch.base.is_running(),
        "base 旗标必须被 stop 清除（manager 守卫判定面）"
    );
}

#[tokio::test]
async fn test_polling_loop_replaces_and_aborts_previous_handle() {
    // 二次 polling_loop 必须 abort 旧任务并替换句柄（restart 竞态的核心
    // 修复面）。JoinHandle 不可 Clone（tokio 全版本），abort 判定用注入的
    // victim 任务（300ms 后发送完成信号）做副作用观察：被生产代码 abort →
    // future drop → done_tx drop → done_rx 收 Canceled；abort 缺失 → victim
    // 完整执行 → 300ms 后收到 Ok(())。#[tokio::test] 默认单线程 runtime，
    // 测试完全离线（victim/poll 任务均无网络依赖）。
    let config = TelegramConfig {
        token: "123456:ABC-DEF".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = TelegramChannel::new(config, tx).unwrap();

    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let victim = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = done_tx.send(());
    });
    *ch.poll_handle.lock() = Some(victim);

    ch.polling_loop(); // 必须 abort victim + 挂新句柄
    assert!(ch.poll_handle.lock().is_some(), "新句柄必须在场");

    match tokio::time::timeout(std::time::Duration::from_millis(800), done_rx).await {
        Ok(Ok(())) => panic!("旧任务必须被 abort——victim 在 300ms 后完成（abort 缺失）"),
        Ok(Err(_cancelled)) => {} // victim 被 abort → done_tx drop：预期
        Err(_elapsed) => panic!("victim 既未完成也未取消，观察窗口设计错误"),
    }

    // 收尾：stop 兜底 abort 新任务。
    ch.stop().await.unwrap();
}

// -----------------------------------------------------------------------
// BUG-20261113-158 回归锁：语音转写临时 ogg 必须在转写后删除（Ok/Err 两臂
// finally 语义）。此前两臂都不删——每条语音消息在系统级 temp
// （std::env::temp_dir/nemesisbot_telegram_voice，不随 workspace 清理）永久
// 残留一个 ogg。mock 服务器两连接 = getFile + 文件下载；MockTranscriber
// 不真读文件（纯返回），落盘字节内容无关紧要。
// -----------------------------------------------------------------------

/// 转写成功臂：临时 ogg 删除。
#[tokio::test]
async fn t15_voice_transcribe_removes_temp_ogg_on_success() {
    let file_id = "nb_t15_voice_ok";
    let base = spawn_mock_telegram_api(b"fake-ogg-bytes".to_vec(), "123456:nb-t15-token");
    let api = TelegramApi::new(&base, "123456:nb-t15-token");
    let transcriber: Arc<dyn crate::base::VoiceTranscriber> = Arc::new(MockTranscriber {
        available: true,
        text: "transcribed".to_string(),
        should_fail: false,
    });
    let client = reqwest::Client::new();

    let result =
        TelegramChannel::transcribe_voice(&Some(transcriber), Some(&client), Some(&api), file_id)
            .await;
    assert_eq!(
        result.as_deref(),
        Some("[voice transcription: transcribed]"),
        "成功臂返回形态"
    );

    let leaked = std::env::temp_dir()
        .join("nemesisbot_telegram_voice")
        .join(format!("{file_id}.ogg"));
    assert!(
        !leaked.exists(),
        "转写成功后临时 ogg 必须被删: {}",
        leaked.display()
    );
}

/// 转写失败臂：临时 ogg 同样删除（finally 语义两侧都锁——失败臂是泄漏
/// 主路径：转写服务异常时每条语音仍落盘）。
#[tokio::test]
async fn t16_voice_transcribe_removes_temp_ogg_on_failure() {
    let file_id = "nb_t16_voice_err";
    let base = spawn_mock_telegram_api(b"fake-ogg-bytes".to_vec(), "123456:nb-t16-token");
    let api = TelegramApi::new(&base, "123456:nb-t16-token");
    let transcriber: Arc<dyn crate::base::VoiceTranscriber> = Arc::new(MockTranscriber {
        available: true,
        text: String::new(),
        should_fail: true,
    });
    let client = reqwest::Client::new();

    let result =
        TelegramChannel::transcribe_voice(&Some(transcriber), Some(&client), Some(&api), file_id)
            .await;
    assert_eq!(
        result.as_deref(),
        Some("[voice (transcription failed)]"),
        "失败臂返回形态"
    );

    let leaked = std::env::temp_dir()
        .join("nemesisbot_telegram_voice")
        .join(format!("{file_id}.ogg"));
    assert!(
        !leaked.exists(),
        "转写失败后临时 ogg 同样必须被删: {}",
        leaked.display()
    );
}

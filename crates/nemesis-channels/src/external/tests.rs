use super::*;

/// W4c：给测试用的消息总线 sender（external 通道入站发布修复后构造函数需要 bus）。
fn w4c_test_bus() -> tokio::sync::broadcast::Sender<InboundMessage> {
    tokio::sync::broadcast::channel(16).0
}

#[test]
fn test_external_channel_new_validates() {
    let config = ExternalConfig {
        input_exe: String::new(),
        output_exe: String::new(),
        chat_id: "default".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    assert!(ExternalChannel::new(config.clone(), w4c_test_bus()).is_err());
}

#[tokio::test]
async fn test_external_channel_lifecycle() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    assert_eq!(ch.name(), "external");

    ch.start().await.unwrap();
    assert!(ch.running.load(Ordering::SeqCst));

    ch.stop().await.unwrap();
    assert!(!ch.running.load(Ordering::SeqCst));
}

#[test]
fn test_process_input_line() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (sender, chat, content) = ch.process_input_line("hello world").unwrap();
    assert_eq!(sender, "test-chat");
    assert_eq!(chat, "test-chat");
    assert_eq!(content, "hello world");
}

#[test]
fn test_process_input_line_empty() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert!(ch.process_input_line("").is_none());
    assert!(ch.process_input_line("   ").is_none());
}

#[test]
fn test_format_output() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert_eq!(ch.format_output("hello"), "hello\n");
}

#[tokio::test]
async fn test_send_validates_chat_id() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "wrong-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[test]
fn test_external_config_accessors() {
    let config = ExternalConfig {
        input_exe: "/path/to/input".to_string(),
        output_exe: "/path/to/output".to_string(),
        chat_id: "my-chat".to_string(),
        sync_to: vec!["web".to_string()],
        allow_from: vec!["user1".to_string()],
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    assert_eq!(ch.input_exe(), "/path/to/input");
    assert_eq!(ch.output_exe(), "/path/to/output");
    assert_eq!(ch.chat_id(), "my-chat");
}

#[test]
fn test_new_requires_input_exe() {
    let config = ExternalConfig {
        input_exe: String::new(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    assert!(ExternalChannel::new(config.clone(), w4c_test_bus()).is_err());
}

#[test]
fn test_new_requires_output_exe() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: String::new(),
        chat_id: "test".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    assert!(ExternalChannel::new(config.clone(), w4c_test_bus()).is_err());
}

#[tokio::test]
async fn test_send_not_running() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    // Not started - should fail
    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "test-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[test]
fn test_process_input_line_whitespace() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (_, _, content) = ch.process_input_line("  hello world  ").unwrap();
    assert_eq!(content, "hello world");
}

// ---- Additional comprehensive external channel tests ----

#[test]
fn test_process_input_line_unicode() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (_, _, content) = ch.process_input_line("你好世界 🌍").unwrap();
    assert_eq!(content, "你好世界 🌍");
}

#[test]
fn test_process_input_line_newlines() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (_, _, content) = ch.process_input_line("line1\nline2").unwrap();
    assert_eq!(content, "line1\nline2");
}

#[test]
fn test_process_input_line_tabs() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (_, _, content) = ch.process_input_line("\thello\t").unwrap();
    assert_eq!(content, "hello");
}

#[test]
fn test_process_input_line_long_line() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let long = "x".repeat(100_000);
    let (_, _, content) = ch.process_input_line(&long).unwrap();
    assert_eq!(content.len(), 100_000);
}

#[test]
fn test_format_output_empty() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert_eq!(ch.format_output(""), "\n");
}

#[test]
fn test_format_output_unicode() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert_eq!(ch.format_output("你好"), "你好\n");
}

#[test]
fn test_process_input_line_returns_chat_id_as_sender() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "my-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (sender, chat, _) = ch.process_input_line("hello").unwrap();
    assert_eq!(sender, "my-chat");
    assert_eq!(chat, "my-chat");
}

#[test]
fn test_new_validates_both_exes() {
    let config = ExternalConfig {
        input_exe: String::new(),
        output_exe: String::new(),
        chat_id: "test".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    // Both empty - should fail
    assert!(ExternalChannel::new(config.clone(), w4c_test_bus()).is_err());
}

#[test]
fn test_sync_to_config() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test".to_string(),
        sync_to: vec!["web".to_string(), "discord".to_string()],
        allow_from: vec!["user1".to_string()],
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    assert_eq!(ch.input_exe(), "/bin/echo");
    assert_eq!(ch.output_exe(), "/bin/cat");
    assert_eq!(ch.chat_id(), "test");
}

#[tokio::test]
async fn test_start_stop_multiple_cycles() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    for _ in 0..3 {
        ch.start().await.unwrap();
        assert!(ch.running.load(Ordering::SeqCst));
        ch.stop().await.unwrap();
        assert!(!ch.running.load(Ordering::SeqCst));
    }
}

// ---- Additional coverage tests ----

#[tokio::test]
async fn test_send_correct_chat_id() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "test-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Should succeed - correct chat_id, spawns output process
    let result = ch.send(msg).await;
    assert!(result.is_ok());
}

#[test]
fn test_process_input_line_special_chars() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (_, _, content) = ch.process_input_line("!@#$%^&*()").unwrap();
    assert_eq!(content, "!@#$%^&*()");
}

#[test]
fn test_format_output_special_chars() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert_eq!(ch.format_output("line1\nline2"), "line1\nline2\n");
}

#[test]
fn test_process_input_line_only_spaces() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert!(ch.process_input_line("     ").is_none());
}

#[test]
fn test_process_input_line_only_tabs() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert!(ch.process_input_line("\t\t\t").is_none());
}

// --- Additional coverage tests ---

#[test]
fn test_external_config_all_fields() {
    let config = ExternalConfig {
        input_exe: "/usr/bin/input".to_string(),
        output_exe: "/usr/bin/output".to_string(),
        chat_id: "my-chat".to_string(),
        sync_to: vec!["web".to_string(), "discord".to_string()],
        allow_from: vec!["admin".to_string()],
    };
    assert_eq!(config.input_exe, "/usr/bin/input");
    assert_eq!(config.output_exe, "/usr/bin/output");
    assert_eq!(config.chat_id, "my-chat");
    assert_eq!(config.sync_to.len(), 2);
    assert_eq!(config.allow_from.len(), 1);
}

#[test]
fn test_process_input_line_with_spaces_and_text() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (sender, chat, content) = ch.process_input_line("  hello world  ").unwrap();
    assert_eq!(content, "hello world");
    assert_eq!(sender, "chat");
    assert_eq!(chat, "chat");
}

#[tokio::test]
async fn test_start_stop_idempotent() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.start().await.unwrap();
    assert!(ch.running.load(Ordering::SeqCst));

    ch.stop().await.unwrap();
    ch.stop().await.unwrap();
    assert!(!ch.running.load(Ordering::SeqCst));
}

#[test]
fn test_format_output_multi_line() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    assert_eq!(
        ch.format_output("line1\nline2\nline3"),
        "line1\nline2\nline3\n"
    );
}

#[test]
fn test_process_input_line_carriage_return() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();

    let (_, _, content) = ch.process_input_line("  hello\r\n  ").unwrap();
    assert_eq!(content, "hello");
}

#[tokio::test]
async fn test_send_with_sync_to_config() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "test-chat".to_string(),
        sync_to: vec!["web".to_string()],
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "test-chat".to_string(),
        content: "sync test".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Should succeed - correct chat_id
    let result = ch.send(msg).await;
    assert!(result.is_ok());
}

// ============================================================
// Additional coverage tests for 95%+ target (round 2)
// ============================================================

#[test]
fn test_external_config_default_fields() {
    let config = ExternalConfig {
        input_exe: "a".to_string(),
        output_exe: "b".to_string(),
        chat_id: "c".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    assert_eq!(ch.name(), "external");
    assert_eq!(ch.chat_id(), "c");
}

#[tokio::test]
async fn test_send_validates_running_state() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    // Never started, so send should fail
    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "chat".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not running"));
}

#[test]
fn test_format_output_multiline_content() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    let output = ch.format_output("line1\nline2\nline3");
    assert!(output.ends_with('\n'));
}

#[tokio::test]
async fn test_send_with_invalid_chat_id_error_message() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "expected-chat".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "wrong-chat-id".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid chat ID"));
    assert!(err.contains("wrong-chat-id"));
    assert!(err.contains("expected-chat"));
}

#[test]
fn test_new_valid_config() {
    let config = ExternalConfig {
        input_exe: "/usr/bin/input".to_string(),
        output_exe: "/usr/bin/output".to_string(),
        chat_id: "chat-1".to_string(),
        sync_to: vec!["web".to_string()],
        allow_from: vec!["user1".to_string()],
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus());
    assert!(ch.is_ok());
    let ch = ch.unwrap();
    assert_eq!(ch.input_exe(), "/usr/bin/input");
    assert_eq!(ch.output_exe(), "/usr/bin/output");
    assert_eq!(ch.chat_id(), "chat-1");
}

// ============================================================
// Additional coverage tests (round 3): cancel-tx stop path, helpers
// ============================================================

#[tokio::test]
async fn test_stop_takes_cancel_tx_after_start() {
    // After start(), spawn_input_reader() installs a cancel_tx sender. Calling
    // stop() must take() it (Some) and send the cancellation signal without
    // error. This exercises the cancel path of stop().
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "stop-test".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();

    // The cancel_tx slot should be populated after start.
    assert!(ch.cancel_tx.lock().is_some());

    ch.stop().await.unwrap();

    // After stop, the cancel_tx sender has been taken out (None).
    assert!(ch.cancel_tx.lock().is_none());
    // input_child slot stays None (spawn_input_reader never populated it).
    assert!(ch.input_child.lock().is_none());
}

#[tokio::test]
async fn test_stop_without_start_takes_none_cancel_tx() {
    // stop() called before start(): cancel_tx slot is None, take() yields None,
    // and input_child is also None. Must still return Ok.
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "nostart".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    assert!(ch.cancel_tx.lock().is_none());

    ch.stop().await.unwrap();
    assert!(!ch.running.load(Ordering::SeqCst));
}

#[test]
fn test_process_input_line_single_char() {
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "c".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    let (s, c, content) = ch.process_input_line("x").unwrap();
    assert_eq!(s, "c");
    assert_eq!(c, "c");
    assert_eq!(content, "x");
}

#[test]
fn test_process_input_line_null_byte_preserved() {
    // A NUL byte inside the (non-empty after trim) content is preserved.
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "c".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    let (_, _, content) = ch.process_input_line("a\u{0}b").unwrap();
    assert_eq!(content, "a\u{0}b");
}

#[test]
fn test_format_output_preserves_existing_trailing_newline() {
    // format_output appends exactly one '\n' regardless of existing newlines.
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "c".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    assert_eq!(ch.format_output("already\n"), "already\n\n");
    assert_eq!(ch.format_output("multi\n\n\n"), "multi\n\n\n\n");
}

#[test]
fn test_process_input_line_preserves_internal_spaces() {
    // Internal (non-edge) whitespace must be preserved, only edges trimmed.
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "c".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    let (_, _, content) = ch.process_input_line("  a   b\tc  ").unwrap();
    assert_eq!(content, "a   b\tc");
}

#[test]
fn test_external_config_clone_is_equal() {
    // ExternalConfig derives Clone; verify a clone matches field-for-field.
    let config = ExternalConfig {
        input_exe: "/in".to_string(),
        output_exe: "/out".to_string(),
        chat_id: "chat".to_string(),
        sync_to: vec!["web".to_string()],
        allow_from: vec!["u1".to_string()],
    };
    let cloned = config.clone();
    assert_eq!(cloned.input_exe, config.input_exe);
    assert_eq!(cloned.output_exe, config.output_exe);
    assert_eq!(cloned.chat_id, config.chat_id);
    assert_eq!(cloned.sync_to, config.sync_to);
    assert_eq!(cloned.allow_from, config.allow_from);
}

#[test]
fn test_process_input_line_returns_same_chat_id_in_both_slots() {
    // The first two tuple slots are both chat_id (sender == chat for external).
    let config = ExternalConfig {
        input_exe: "/bin/echo".to_string(),
        output_exe: "/bin/cat".to_string(),
        chat_id: "dup-check".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    let (a, b, _) = ch.process_input_line("payload").unwrap();
    assert_eq!(a, b);
    assert_eq!(a, "dup-check");
}

// ===========================================================================
// W4c 补测（2026-08-25）：BUG #10 回归测试（input EXE 行发布到 bus——修复前
// 只 debug 日志直接丢弃，Go 原版 external.go:192 HandleMessage 有发布）+ 允许列表
// 拦截 + sync_to_targets 联动 + send 真写 output EXE stdin + 长驻 input 的 cancel 臂
// ===========================================================================

#[cfg(target_os = "windows")]
fn w4c_write_bat(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("{}_{}.bat", name, std::process::id()));
    std::fs::write(&p, body).unwrap();
    p
}

/// BUG #10 回归：input EXE 输出的每一行必须作为 InboundMessage 发布到 bus。
#[tokio::test]
async fn test_w4c_external_input_publishes_to_bus() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("w4c_ext_in", "@echo off\r\necho hello-from-input\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    let recv = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();

    let inbound = recv
        .expect("timed out: input EXE line was never published to the bus")
        .expect("broadcast receive failed");
    assert_eq!(inbound.channel, "external");
    assert_eq!(inbound.chat_id, "ext-chat");
    assert_eq!(inbound.sender_id, "ext-chat");
    assert_eq!(inbound.content, "hello-from-input");
}

/// 允许列表不含 chat_id → 行被拦（不发布）。
#[tokio::test]
async fn test_w4c_external_input_allow_list_blocks() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("w4c_ext_in_block", "@echo off\r\necho blocked-line\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out_block", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec!["someone-else".to_string()],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    let recv = tokio::time::timeout(std::time::Duration::from_millis(1500), rx.recv()).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();

    assert!(
        recv.is_err() || recv.unwrap().is_err(),
        "message must be blocked by allow-list"
    );
}

/// 简单桩通道：记录 send() 到的内容（用于验证 sync_to_targets 联动）。
struct W4cSyncStub {
    name: String,
    received: Arc<parking_lot::RwLock<Vec<String>>>,
}

#[async_trait]
impl Channel for W4cSyncStub {
    fn name(&self) -> &str {
        &self.name
    }
    fn is_running(&self) -> bool {
        true
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        self.received.write().push(msg.content.clone());
        Ok(())
    }
}

/// input EXE 行发布到 bus 的同时必须转发到已注册的 sync target（Go SyncToTargets 对齐）。
#[tokio::test]
async fn test_w4c_external_input_syncs_to_targets() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("w4c_ext_in_sync", "@echo off\r\necho sync-me\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out_sync", "@echo off\r\n");

    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();

    let stub_received = Arc::new(parking_lot::RwLock::new(Vec::new()));
    let stub = Arc::new(W4cSyncStub {
        name: "w4c-tgt".to_string(),
        received: stub_received.clone(),
    });
    ch.base.add_sync_target("w4c-tgt", stub).unwrap();

    ch.start().await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut got = Vec::new();
    while std::time::Instant::now() < deadline {
        got = stub_received.read().clone();
        if !got.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();

    assert_eq!(got, vec!["sync-me".to_string()]);
}

/// send() 真正把内容写进 output EXE 的 stdin（批处理读一行并落盘验证）。
#[tokio::test]
async fn test_w4c_external_send_writes_to_output_exe_stdin() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("w4c_ext_in_send", "@echo off\r\n");
    let proof = std::env::temp_dir().join(format!("w4c_ext_proof_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&proof);
    let body = format!(
        "@echo off\r\nset /p line=\r\necho %line%>> \"{}\"\r\n",
        proof.to_string_lossy()
    );
    let out_bat = w4c_write_bat("w4c_ext_out_send", &body);

    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    ch.send(OutboundMessage::new("ext-chat", "ext-chat", "proof-line"))
        .await
        .unwrap();

    // 轮询落盘文件（send 内部 spawn 异步任务）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut content = String::new();
    while std::time::Instant::now() < deadline {
        if let Ok(s) = std::fs::read_to_string(&proof)
            && !s.trim().is_empty() {
                content = s;
                break;
            }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    let _ = std::fs::remove_file(&proof);
    ch.stop().await.unwrap();

    assert!(
        content.trim() == "proof-line",
        "output EXE should have received the content via stdin; got {:?}",
        content
    );
}

/// 长驻 input EXE（stdout 无输出）→ stop() 走 cancel 臂杀进程，不挂起。
#[tokio::test]
async fn test_w4c_external_stop_cancels_long_running_input() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("w4c_ext_in_hang", "@echo off\r\nping -n 60 127.0.0.1 > nul\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out_hang", "@echo off\r\n");

    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), ch.stop()).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    stopped
        .expect("stop() must not hang on long-running input EXE")
        .unwrap();
}

// ===========================================================================
// S2 coverage (2026-08-26): no-receiver publish warn arm / empty-line skip /
// Channel::is_running trait impl / output EXE broken-pipe write error arm
// ===========================================================================

/// Input EXE emits a line but the bus has zero receivers -> `bus_sender.send`
/// returns Err and the warn arm fires (line is still consumed, no panic).
#[tokio::test]
async fn s2_external_input_publish_with_no_bus_receivers_logs_warn() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("s2_ext_in_norx", "@echo off\r\necho orphan-line\r\n");
    let out_bat = w4c_write_bat("s2_ext_out_norx", "@echo off\r\n");

    let (bus, rx) = tokio::sync::broadcast::channel::<InboundMessage>(16);
    drop(rx); // zero receivers on purpose
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "s2-norx".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // Let the reader task consume the line and hit the send-Err arm.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();
}

/// Input EXE prints an empty line -> `trimmed.is_empty()` edge: line is
/// skipped (no publish) and the loop continues via `line.clear()`.
#[tokio::test]
async fn s2_external_input_empty_line_is_skipped() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    // `echo.` prints exactly one empty line.
    let in_bat = w4c_write_bat("s2_ext_in_empty", "@echo off\r\n@echo.\r\n");
    let out_bat = w4c_write_bat("s2_ext_out_empty", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel::<InboundMessage>(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "s2-empty".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // The empty line must NOT publish anything to the bus.
    let recv = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();
    assert!(recv.is_err(), "empty line must not be published, got {:?}", recv.ok());
}

/// The Channel trait's is_running impl was never called directly.
#[tokio::test]
async fn s2_external_trait_is_running_call() {
    let (bus, _rx) = tokio::sync::broadcast::channel::<InboundMessage>(16);
    let config = ExternalConfig {
        input_exe: "whatever.exe".to_string(),
        output_exe: "whatever.exe".to_string(),
        chat_id: "s2-ir".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    assert!(!ch.is_running());
}

/// Output EXE exits immediately without reading stdin; a payload larger than
/// the OS pipe buffer then fails `write_all` (broken pipe) and the error arm
/// fires. send() itself stays Ok (the write happens in a spawned task).
#[tokio::test]
async fn s2_external_send_broken_pipe_on_output_exe_logs_error() {
    if !cfg!(target_os = "windows") {
        eprintln!("Skipping: windows-only .bat fixture");
        return;
    }
    let in_bat = w4c_write_bat("s2_ext_in_pipe", "@echo off\r\n");
    // `exit` closes stdin reader side immediately.
    let out_bat = w4c_write_bat("s2_ext_out_pipe", "@exit\r\n");

    let (bus, _rx) = tokio::sync::broadcast::channel::<InboundMessage>(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "s2-pipe".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // >64KB (Windows pipe buffer) so the write cannot complete into a pipe
    // whose reader has already exited.
    let big = "x".repeat(100_000);
    let msg = OutboundMessage::new("external", "s2-pipe", &big);
    ch.send(msg).await.unwrap();

    // Let the spawned writer hit the broken pipe.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();
}

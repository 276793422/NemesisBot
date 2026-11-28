use super::*;

/// 给测试用的消息总线 sender（external 通道入站发布修复后构造函数需要 bus）。
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    ch.start().await.unwrap();

    // The cancel_tx slot should be populated after start.
    assert!(ch.cancel_tx.lock().is_some());

    ch.stop().await.unwrap();

    // After stop, the cancel_tx sender has been taken out (None).
    assert!(ch.cancel_tx.lock().is_none());
    // BUG-20261128-032：input_child 死字段已删（spawn 后 child 生命周期全程
    // 在 reader task 内，字段从未被写入过，stop 的 kill 死路径由 cancel 臂
    // 承担）——残留引用同步清除。
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
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
        working_dir: None,
    };
    let ch = ExternalChannel::new(config.clone(), w4c_test_bus()).unwrap();
    let (a, b, _) = ch.process_input_line("payload").unwrap();
    assert_eq!(a, b);
    assert_eq!(a, "dup-check");
}

// ===========================================================================
// 补测： 回归测试（input EXE 行发布到 bus——修复前
// 只 debug 日志直接丢弃，Go 原版 external.go:192 HandleMessage 有发布）+ 允许列表
// 拦截 + sync_to_targets 联动 + send 真写 output EXE stdin + 长驻 input 的 cancel 臂
// ===========================================================================

#[cfg(target_os = "windows")]
fn w4c_write_bat(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("{}_{}.bat", name, std::process::id()));
    std::fs::write(&p, body).unwrap();
    p
}

/// 回归：input EXE 输出的每一行必须作为 InboundMessage 发布到 bus。
#[cfg(target_os = "windows")]
#[tokio::test]
async fn test_w4c_external_input_publishes_to_bus() {
    let in_bat = w4c_write_bat("w4c_ext_in", "@echo off\r\necho hello-from-input\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
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
#[cfg(target_os = "windows")]
#[tokio::test]
async fn test_w4c_external_input_allow_list_blocks() {
    let in_bat = w4c_write_bat("w4c_ext_in_block", "@echo off\r\necho blocked-line\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out_block", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec!["someone-else".to_string()],
        working_dir: None,
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
// Only the windows-gated sync test below constructs it; gate with the same
// cfg so other targets don't see it as dead code (Linux CI clippy -D warnings).
#[cfg(target_os = "windows")]
struct W4cSyncStub {
    name: String,
    received: Arc<parking_lot::RwLock<Vec<String>>>,
}

#[cfg(target_os = "windows")]
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
#[cfg(target_os = "windows")]
#[tokio::test]
async fn test_w4c_external_input_syncs_to_targets() {
    let in_bat = w4c_write_bat("w4c_ext_in_sync", "@echo off\r\necho sync-me\r\n");
    let out_bat = w4c_write_bat("w4c_ext_out_sync", "@echo off\r\n");

    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
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
#[cfg(target_os = "windows")]
#[tokio::test]
async fn test_w4c_external_send_writes_to_output_exe_stdin() {
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
        working_dir: None,
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
            && !s.trim().is_empty()
        {
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
#[cfg(target_os = "windows")]
#[tokio::test]
async fn test_w4c_external_stop_cancels_long_running_input() {
    let in_bat = w4c_write_bat(
        "w4c_ext_in_hang",
        "@echo off\r\nping -n 60 127.0.0.1 > nul\r\n",
    );
    let out_bat = w4c_write_bat("w4c_ext_out_hang", "@echo off\r\n");

    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "ext-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
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
// coverage: no-receiver publish warn arm / empty-line skip /
// Channel::is_running trait impl / output EXE broken-pipe write error arm
// ===========================================================================

/// Input EXE emits a line but the bus has zero receivers -> `bus_sender.send`
/// returns Err and the warn arm fires (line is still consumed, no panic).
#[cfg(target_os = "windows")]
#[tokio::test]
async fn s2_external_input_publish_with_no_bus_receivers_logs_warn() {
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
        working_dir: None,
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
#[cfg(target_os = "windows")]
#[tokio::test]
async fn s2_external_input_empty_line_is_skipped() {
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
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // The empty line must NOT publish anything to the bus.
    let recv = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();
    assert!(
        recv.is_err(),
        "empty line must not be published, got {:?}",
        recv.ok()
    );
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
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    assert!(!ch.is_running());
}

/// Output EXE exits immediately without reading stdin; a payload larger than
/// the OS pipe buffer then fails `write_all` (broken pipe) and the error arm
/// fires. send() itself stays Ok (the write happens in a spawned task).
#[cfg(target_os = "windows")]
#[tokio::test]
async fn s2_external_send_broken_pipe_on_output_exe_logs_error() {
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
        working_dir: None,
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

// ===========================================================================
// BUG-20261123-002 同族回归锁：input EXE 的 stderr 必须有排水任务——
// 旧形态 stderr piped 无消费者，长寿命 input EXE stderr 洪泛（> 管道缓冲）
// 后写阻塞冻死，stdout 的 READY 行永不到达总线（通道静默瘫痪判别面）。
// 修复后 stderr 排水与 stdout 读并发，READY 照常发布。
// Windows 臂用 .bat；Unix 臂 sh 脚本（Linux nightly 兜底）。
// ===========================================================================

#[cfg(target_os = "windows")]
#[tokio::test]
async fn input_exe_stderr_flood_does_not_block_stdout_lines_windows() {
    let in_bat = w4c_write_bat(
        "w4c_ext_stderr_flood_in",
        "@echo off\r\nfor /L %%i in (1,1,4000) do @echo 0123456789012345678901234567890123456789 1>&2\r\necho READY\r\nping -n 30 127.0.0.1 >nul\r\n",
    );
    let out_bat = w4c_write_bat("w4c_ext_stderr_flood_out", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "w4c-flood".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // READY 行必须在窗口内到达（洪灌 ~164KB 先行，排水并发收走）。
    let mut got = None;
    for _ in 0..150 {
        match rx.try_recv() {
            Ok(msg) => {
                got = Some(msg);
                break;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(_) => break,
        }
    }
    let msg = got
        .expect("READY line never arrived: input EXE wedged writing to an undrained stderr pipe");
    assert_eq!(msg.content, "READY");

    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    ch.stop().await.unwrap();
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn input_exe_stderr_flood_does_not_block_stdout_lines_unix() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("input_flood.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nhead -c 200000 /dev/zero 1>&2\necho READY\nsleep 30\n",
    )
    .unwrap();
    // BUG-20261128-005：fs::write 落 0644 无执行位——Linux execve 直接
    // EACCES（std 只对 ENOEXEC 回落 /bin/sh，EACCES 原样报错）→ input EXE
    // spawn 失败 → READY 永不到（coverage 五连红才见光的潜伏死）。+x 后
    // 无 shebang 亦走 sh 回落；shebang 一并补上双保险。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: script.to_string_lossy().to_string(),
        output_exe: "true".to_string(),
        chat_id: "flood-chat".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    let mut got = None;
    for _ in 0..150 {
        match rx.try_recv() {
            Ok(msg) => {
                got = Some(msg);
                break;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(_) => break,
        }
    }
    let msg = got
        .expect("READY line never arrived: input EXE wedged writing to an undrained stderr pipe");
    assert_eq!(msg.content, "READY");
    ch.stop().await.unwrap();
}

/// BUG-20261125-019 回归锁：start/stop 必须维护 base running 旗标 + 幂等双启
/// 守卫（BUG-20261121-050 同构家族横向扫漏网补齐——manager 的 start_all 守卫 /
/// any_channel_running / status 上报三消费点都读 trait is_running）。
/// 服务端点指 loopback 死端口（后台 receive loop 连接失败零外网，start 本体
/// 不阻塞）；二次 start 走幂等守卫诚实跳过（不 spawn 第二个 receive loop）。
#[tokio::test]
async fn b019_start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let config = ExternalConfig {
        input_exe: "nonexistent-input.exe".to_string(),
        output_exe: "nonexistent-output.exe".to_string(),
        chat_id: "test".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
        working_dir: None,
    };
    let (tx, _) = tokio::sync::broadcast::channel(16);
    let ch = ExternalChannel::new(config, tx).unwrap();
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

// ===========================================================================
// BUG-20261128-032 回归锁：input EXE 意外死亡（非零退出码）曾被当正常关停
// （INFO + break，无重启），通道 input 面静默死亡、后续消息全丢、base running
// 旗标恒 true（半死状态无人知晓）。实证根因：Windows python 管道 stdout 默认
// locale 编码，`print("你好")` 直接 UnicodeEncodeError 退出（rc=1）。
// 修复后语义：非停机期死亡带退避自动重启；连续快速死亡达上限放弃并诚实翻转
// base running 旗标。两则测试在旧实现上都红（重启永不发生 / 旗标永不翻）。
// ===========================================================================

/// 哨兵路径（测试进程 pid 隔离，防并行测试互踩）。
#[cfg(target_os = "windows")]
fn b032_win_sentinel(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("w4c_b032_{}_{}.flg", tag, std::process::id()))
}

/// 回归锁 ①：input EXE 第一次异常死亡（打印 ready1 后 exit 1）→ 框架自动
/// 重启 → 第二次实例打印 ready2 后挂住 → 两条都必须到达 bus。
/// 旧实现：ready1 后 EOF 即静默 break，ready2 永不到达（旧红）。
#[cfg(target_os = "windows")]
#[tokio::test]
async fn b032_input_exe_abnormal_death_is_restarted_windows() {
    let sentinel = b032_win_sentinel("restart");
    let _ = std::fs::remove_file(&sentinel);
    let in_bat = w4c_write_bat(
        "w4c_b032_restart_in",
        format!(
            "@echo off\r\nif exist \"{}\" goto second\r\necho first> \"{}\"\r\necho ready1\r\nexit /b 1\r\n:second\r\necho ready2\r\nping -n 60 127.0.0.1 >nul\r\n",
            sentinel.to_string_lossy(),
            sentinel.to_string_lossy()
        )
        .as_str(),
    );
    let out_bat = w4c_write_bat("w4c_b032_restart_out", "@echo off\r\n");

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "b032-restart".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // 收两条消息（ready1 + ready2），总窗口 20s（含 1s 重启退避 + spawn 开销）。
    let mut got = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && got.len() < 2 {
        match rx.try_recv() {
            Ok(msg) => got.push(msg.content),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(_) => break,
        }
    }
    // 重启自愈后通道全功能旗标保持 true（必须在 stop 前采样：stop 本身会翻 false）。
    let still_running = ch.base.is_running();
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);
    let _ = std::fs::remove_file(&sentinel);
    ch.stop().await.unwrap();

    assert_eq!(
        got,
        vec!["ready1".to_string(), "ready2".to_string()],
        "input EXE 异常死亡后必须自动重启：ready2 未到达说明重启未发生（BUG-20261128-032）"
    );
    assert!(
        still_running,
        "重启自愈后 base running 旗标必须保持 true（BUG-20261128-032）"
    );
}

/// 回归锁 ②：input EXE 连续秒死（exit 1）达上限 → 放弃重启 + base running
/// 旗标诚实翻转（manager status 上报可见）；`running` AtomicBool（出站 send
/// 面）保持 true（半死通道出站语义独立）。旧实现旗标恒 true（旧红）。
#[cfg(target_os = "windows")]
#[tokio::test]
async fn b032_consecutive_fast_deaths_flip_running_flag_windows() {
    let in_bat = w4c_write_bat("w4c_b032_giveup_in", "@echo off\r\nexit /b 1\r\n");
    let out_bat = w4c_write_bat("w4c_b032_giveup_out", "@echo off\r\n");

    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: in_bat.to_string_lossy().to_string(),
        output_exe: out_bat.to_string_lossy().to_string(),
        chat_id: "b032-giveup".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    // 时序预算：5 次 spawn（秒死）+ 4 次退避 1+2+4+8 = 15s + spawn 开销。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if !ch.base.is_running() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let _ = std::fs::remove_file(&in_bat);
    let _ = std::fs::remove_file(&out_bat);

    assert!(
        !ch.base.is_running(),
        "连续快速死亡达上限后 base running 旗标必须翻转（放弃重启的诚实状态）"
    );
    assert!(
        ch.running.load(Ordering::SeqCst),
        "出站 send 面的 running AtomicBool 与 input 面语义独立，保持 true"
    );
    ch.stop().await.unwrap();
}

/// 回归锁 ① Unix 臂（Linux nightly 兜底）。
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn b032_input_exe_abnormal_death_is_restarted_unix() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let sentinel = tmp.path().join("sentinel.flg");
    let script = tmp.path().join("input_restart.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ -f \"{}\" ]; then echo ready2; sleep 60; else echo first > \"{}\"; echo ready1; exit 1; fi\n",
            sentinel.to_string_lossy(),
            sentinel.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (bus, mut rx) = tokio::sync::broadcast::channel(16);
    let config = ExternalConfig {
        input_exe: script.to_string_lossy().to_string(),
        output_exe: "true".to_string(),
        chat_id: "b032-restart".to_string(),
        sync_to: vec![],
        allow_from: vec![],
        working_dir: None,
    };
    let ch = ExternalChannel::new(config, bus).unwrap();
    ch.start().await.unwrap();

    let mut got = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && got.len() < 2 {
        match rx.try_recv() {
            Ok(msg) => got.push(msg.content),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(_) => break,
        }
    }
    // BUG-20261128-033：健康断言必须在 stop 之前——stop() 无条件翻 base
    // 旗标为 false（见 Channel::stop 顺序纪律），stop 后断言 is_running()
    // 恒假，Linux CI 首跑即确定性红。
    assert!(
        ch.base.is_running(),
        "两条消息都到、重启循环存活时 base running 旗标必须为 true"
    );
    ch.stop().await.unwrap();

    assert_eq!(
        got,
        vec!["ready1".to_string(), "ready2".to_string()],
        "input EXE 异常死亡后必须自动重启：ready2 未到达说明重启未发生（BUG-20261128-032）"
    );
}

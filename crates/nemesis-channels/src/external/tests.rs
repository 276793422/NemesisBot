use super::*;

#[test]
fn test_external_channel_new_validates() {
    let config = ExternalConfig {
        input_exe: String::new(),
        output_exe: String::new(),
        chat_id: "default".to_string(),
        sync_to: Vec::new(),
        allow_from: Vec::new(),
    };
    assert!(ExternalChannel::new(config).is_err());
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
    let ch = ExternalChannel::new(config).unwrap();
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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "wrong-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
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
    let ch = ExternalChannel::new(config).unwrap();
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
    assert!(ExternalChannel::new(config).is_err());
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
    assert!(ExternalChannel::new(config).is_err());
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
    let ch = ExternalChannel::new(config).unwrap();
    // Not started - should fail
    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "test-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    assert!(ExternalChannel::new(config).is_err());
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
    let ch = ExternalChannel::new(config).unwrap();
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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "test-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();
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
    let ch = ExternalChannel::new(config).unwrap();

    assert_eq!(ch.format_output("line1\nline2\nline3"), "line1\nline2\nline3\n");
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
    let ch = ExternalChannel::new(config).unwrap();

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
    let ch = ExternalChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "test-chat".to_string(),
        content: "sync test".to_string(),
        message_type: String::new(),
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
    let ch = ExternalChannel::new(config).unwrap();
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
    let ch = ExternalChannel::new(config).unwrap();
    // Never started, so send should fail
    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "chat".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
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
    let ch = ExternalChannel::new(config).unwrap();
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
    let ch = ExternalChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "external".to_string(),
        chat_id: "wrong-chat-id".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
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
    let ch = ExternalChannel::new(config);
    assert!(ch.is_ok());
    let ch = ch.unwrap();
    assert_eq!(ch.input_exe(), "/usr/bin/input");
    assert_eq!(ch.output_exe(), "/usr/bin/output");
    assert_eq!(ch.chat_id(), "chat-1");
}

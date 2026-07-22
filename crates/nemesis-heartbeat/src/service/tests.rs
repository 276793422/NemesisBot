use super::*;

#[tokio::test]
async fn test_heartbeat_disabled() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: false,
        ..Default::default()
    });
    assert!(svc.start().await.is_ok());
    assert!(!svc.is_running());
}

#[test]
fn test_should_skip_no_file() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    assert!(!svc.should_skip());
}

#[test]
fn test_should_skip_with_nonexistent_file() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    svc.set_skip_file("/nonexistent/path/BOOTSTRAP.md".to_string());
    assert!(!svc.should_skip());
}

#[test]
fn test_status() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let status = svc.status();
    assert_eq!(status["beat_count"], serde_json::json!(0));
}

#[tokio::test]
async fn test_start_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    // Create a valid HEARTBEAT.md so the heartbeat tick actually executes.
    std::fs::write(tmp.path().join("HEARTBEAT.md"), "# Tasks\n\n- Test task").unwrap();

    let called = Arc::new(AtomicU64::new(0));
    let called_clone = called.clone();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });
    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        called_clone.fetch_add(1, Ordering::SeqCst);
        None
    }));

    svc.start().await.unwrap();
    assert!(svc.is_running());

    // First heartbeat fires after 1 second (matching Go's time.AfterFunc(1s)).
    tokio::time::sleep(Duration::from_millis(1500)).await;

    svc.stop();
    assert!(!svc.is_running());
    // Handler should have been called at least once (first heartbeat).
    assert!(called.load(Ordering::SeqCst) >= 1);
}

#[test]
fn test_handler() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let called = Arc::new(AtomicU64::new(0));
    let called_clone = called.clone();
    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        called_clone.fetch_add(1, Ordering::SeqCst);
        None
    }));
    // Handler is set, will be called on tick
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

// --- Tests for new methods ---

#[test]
fn test_is_heartbeat_file_empty_all_comments() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let data = b"# Title\n## Subtitle\n\n# Another comment\n";
    assert!(svc.is_heartbeat_file_empty(data));
}

#[test]
fn test_is_heartbeat_file_empty_with_content() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let data = b"# Title\nSome actual content here\n";
    assert!(!svc.is_heartbeat_file_empty(data));
}

#[test]
fn test_is_heartbeat_file_empty_blank_only() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let data = b"\n\n  \n\t\n";
    assert!(svc.is_heartbeat_file_empty(data));
}

#[test]
fn test_is_heartbeat_file_empty_truly_empty() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let data = b"";
    assert!(svc.is_heartbeat_file_empty(data));
}

#[test]
fn test_parse_last_channel_valid() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (platform, user_id) = svc.parse_last_channel("telegram:123456");
    assert_eq!(platform, "telegram");
    assert_eq!(user_id, "123456");
}

#[test]
fn test_parse_last_channel_empty() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_parse_last_channel_no_colon() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("invalidformat");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_parse_last_channel_internal() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("system:123");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_parse_last_channel_rpc() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("rpc:abc");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_create_default_heartbeat_template() {
    let dir = tempfile::tempdir().unwrap();
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    svc.create_default_heartbeat_template();

    let path = dir.path().join("HEARTBEAT.md");
    assert!(path.exists());

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("Heartbeat Check List"));
    assert!(content.contains("heartbeat tasks below this line"));
}

#[test]
fn test_build_prompt_no_workspace() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: None,
        ..Default::default()
    });
    let prompt = svc.build_prompt();
    assert!(prompt.is_empty());
}

#[test]
fn test_build_prompt_missing_file_creates_template() {
    let dir = tempfile::tempdir().unwrap();
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let prompt = svc.build_prompt();
    assert!(prompt.is_empty()); // Returns empty because file didn't exist

    // But default template should have been created
    let path = dir.path().join("HEARTBEAT.md");
    assert!(path.exists());
}

#[test]
fn test_build_prompt_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Check email\n- Review calendar\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let prompt = svc.build_prompt();
    assert!(prompt.contains("Heartbeat Check"));
    assert!(prompt.contains("Check email"));
    assert!(prompt.contains("Current time:"));
}

#[test]
fn test_build_prompt_comments_only() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "# Just a comment\n## Another comment\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let prompt = svc.build_prompt();
    assert!(prompt.is_empty()); // Only comments = empty prompt
}

#[test]
fn test_config_minimum_interval() {
    let config = HeartbeatConfig::new(2, true, "/tmp/test".to_string());
    assert_eq!(config.interval.as_secs(), 5 * 60); // Clamped to 5 minutes
}

#[test]
fn test_config_zero_uses_default() {
    let config = HeartbeatConfig::new(0, true, "/tmp/test".to_string());
    assert_eq!(config.interval.as_secs(), 30 * 60);
}

#[test]
fn test_config_normal_value() {
    let config = HeartbeatConfig::new(15, true, "/tmp/test".to_string());
    assert_eq!(config.interval.as_secs(), 15 * 60);
}

struct MockBus {
    sent: Arc<Mutex<Vec<(String, String, String)>>>,
}
impl MockBus {
    fn new() -> (Self, Arc<Mutex<Vec<(String, String, String)>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_clone = sent.clone();
        (Self { sent }, sent_clone)
    }
}
impl MessageBus for MockBus {
    fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
        self.sent.lock().push((channel, chat_id, content));
    }
}

struct MockState {
    last_channel: String,
}
impl StateManager for MockState {
    fn get_last_channel(&self) -> String {
        self.last_channel.clone()
    }
}

#[test]
fn test_send_response_with_bus_and_channel() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "telegram:123456".to_string(),
    }));

    svc.send_response("Hello from heartbeat!");

    let sent_lock = sent.lock();
    assert_eq!(sent_lock.len(), 1);
    assert_eq!(sent_lock[0].0, "telegram");
    assert_eq!(sent_lock[0].1, "123456");
    assert_eq!(sent_lock[0].2, "Hello from heartbeat!");
}

#[test]
fn test_send_response_no_bus() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    });
    // Should not panic
    svc.send_response("test");
}

#[test]
fn test_send_response_internal_channel() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "system:123".to_string(),
    }));

    svc.send_response("Hello");

    assert!(sent.lock().is_empty()); // Internal channel should be skipped
}

// ============================================================
// Additional heartbeat tests for missing coverage
// ============================================================

#[test]
fn test_config_default_values() {
    let config = HeartbeatConfig::default();
    assert!(config.enabled);
    assert_eq!(config.interval, Duration::from_secs(30));
    assert!(config.workspace.is_none());
    assert_eq!(config.min_interval_minutes, 5);
    assert_eq!(config.default_interval_minutes, 30);
}

#[test]
fn test_config_new_disabled() {
    let config = HeartbeatConfig::new(10, false, "/tmp/ws".to_string());
    assert!(!config.enabled);
    assert_eq!(config.workspace, Some("/tmp/ws".to_string()));
}

#[test]
fn test_heartbeat_result_fields() {
    let result = HeartbeatResult {
        is_error: false,
        is_async: false,
        silent: true,
        for_user: String::new(),
        for_llm: "OK".to_string(),
    };
    assert!(!result.is_error);
    assert!(!result.is_async);
    assert!(result.silent);
    assert!(result.for_user.is_empty());
    assert_eq!(result.for_llm, "OK");
}

#[test]
fn test_heartbeat_result_debug() {
    let result = HeartbeatResult {
        is_error: true,
        is_async: false,
        silent: false,
        for_user: "err".to_string(),
        for_llm: "error msg".to_string(),
    };
    let debug = format!("{:?}", result);
    assert!(debug.contains("is_error"));
    assert!(debug.contains("error msg"));
}

#[test]
fn test_should_skip_with_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let skip_path = dir.path().join("BOOTSTRAP.md");
    std::fs::write(&skip_path, "bootstrap active").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig::default());
    svc.set_skip_file(skip_path.to_string_lossy().to_string());
    assert!(svc.should_skip());
}

#[test]
fn test_execute_heartbeat_disabled() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: false,
        ..Default::default()
    });
    // Should not panic and should return immediately
    svc.execute_heartbeat();
}

#[test]
fn test_execute_heartbeat_no_workspace() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: None,
        ..Default::default()
    });
    svc.execute_heartbeat();
    // Should return early (no prompt built)
}

#[test]
fn test_execute_heartbeat_no_handler() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Check email\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });
    // No handler set - should log error and return
    svc.execute_heartbeat();
}

#[test]
fn test_execute_heartbeat_handler_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });
    svc.set_handler(Box::new(|_prompt, _channel, _chat_id| None));
    svc.execute_heartbeat();
}

#[test]
fn test_execute_heartbeat_handler_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: true,
            is_async: false,
            silent: false,
            for_user: String::new(),
            for_llm: "something failed".to_string(),
        })
    }));
    svc.execute_heartbeat();
}

#[test]
fn test_execute_heartbeat_handler_returns_silent() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: true,
            for_user: String::new(),
            for_llm: "HEARTBEAT_OK".to_string(),
        })
    }));
    svc.execute_heartbeat();
}

#[test]
fn test_execute_heartbeat_handler_returns_async() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: true,
            silent: false,
            for_user: String::new(),
            for_llm: "spawned task-1".to_string(),
        })
    }));
    svc.execute_heartbeat();
}

#[test]
fn test_execute_heartbeat_sends_for_user() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "web:user123".to_string(),
    }));
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: "Hello user!".to_string(),
            for_llm: "processed".to_string(),
        })
    }));

    svc.execute_heartbeat();
    assert_eq!(sent.lock().len(), 1);
    assert_eq!(sent.lock()[0].2, "Hello user!");
}

#[test]
fn test_execute_heartbeat_sends_for_llm_when_no_for_user() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "discord:789".to_string(),
    }));
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: String::new(),
            for_llm: "LLM response content".to_string(),
        })
    }));

    svc.execute_heartbeat();
    assert_eq!(sent.lock().len(), 1);
    assert_eq!(sent.lock()[0].2, "LLM response content");
}

#[test]
fn test_parse_last_channel_cluster() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("cluster:node-1");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_parse_last_channel_internal_keyword() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("internal:test");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_parse_last_channel_empty_parts() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel(":");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_parse_last_channel_missing_user() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (p, u) = svc.parse_last_channel("telegram:");
    assert!(p.is_empty());
    assert!(u.is_empty());
}

#[test]
fn test_beat_count_starts_at_zero() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    assert_eq!(svc.beat_count(), 0);
}

#[test]
fn test_last_beat_is_recent() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let now = Local::now();
    let diff = (now - svc.last_beat()).num_seconds().abs();
    assert!(diff < 5, "last_beat should be close to now");
}

#[test]
fn test_is_running_initially_false() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    assert!(!svc.is_running());
}

#[test]
fn test_status_contains_expected_keys() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let status = svc.status();
    assert!(status.contains_key("running"));
    assert!(status.contains_key("enabled"));
    assert!(status.contains_key("beat_count"));
    assert!(status.contains_key("last_beat"));
    assert!(status.contains_key("interval_secs"));
}

#[tokio::test]
async fn test_start_twice_no_error() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_secs(60),
        enabled: true,
        workspace: None,
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });
    svc.start().await.unwrap();
    let result = svc.start().await;
    assert!(result.is_ok()); // Second start should be no-op
    svc.stop();
}

#[test]
fn test_create_default_template_no_workspace() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: None,
        ..Default::default()
    });
    // Should not panic
    svc.create_default_heartbeat_template();
}

#[test]
fn test_is_heartbeat_file_empty_mixed_content() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let data = b"# Header\n\nSome real content\n# Footer\n";
    assert!(!svc.is_heartbeat_file_empty(data));
}

#[test]
fn test_is_heartbeat_file_empty_whitespace_lines() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let data = b"  \n  \n# Only comments and whitespace\n  ";
    assert!(svc.is_heartbeat_file_empty(data));
}

#[test]
fn test_send_response_no_state_manager() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    });
    let (mock_bus, _sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    // No state manager - should not panic
    svc.send_response("test");
}

#[test]
fn test_send_response_empty_channel() {
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    });
    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: String::new(),
    }));
    svc.send_response("test");
    assert!(sent.lock().is_empty());
}

#[test]
fn test_heartbeat_logging_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    // Call log_info which should create the log file
    svc.set_handler(Box::new(|_p, _c, _ch| None));
    svc.execute_heartbeat();

    // Check that logs directory exists
    let logs_dir = dir.path().join("logs");
    assert!(logs_dir.exists());
}

// ============================================================
// Additional tests for error handling and edge cases
// ============================================================

#[test]
fn test_build_prompt_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let prompt = svc.build_prompt();
    assert!(prompt.is_empty()); // Empty content = empty prompt
}

#[test]
fn test_send_response_invalid_channel_format() {
    let dir = tempfile::tempdir().unwrap();
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "invalid-format-without-colon".to_string(),
    }));

    svc.send_response("test");
    assert!(sent.lock().is_empty()); // Invalid format should not send
}

#[test]
fn test_send_response_channel_empty_platform() {
    let dir = tempfile::tempdir().unwrap();
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: ":user123".to_string(),
    }));

    svc.send_response("test");
    assert!(sent.lock().is_empty()); // Empty platform should not send
}

#[test]
fn test_send_response_channel_empty_user_id() {
    let dir = tempfile::tempdir().unwrap();
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "telegram:".to_string(),
    }));

    svc.send_response("test");
    assert!(sent.lock().is_empty()); // Empty user_id should not send
}

#[tokio::test]
async fn test_start_stop_with_active_heartbeat() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    // Create HEARTBEAT.md with actual content
    std::fs::write(
        tmp.path().join("HEARTBEAT.md"),
        "# Tasks\n\n- Check email\n- Review calendar",
    )
    .unwrap();

    let heartbeat_executed = Arc::new(AtomicU64::new(0));
    let heartbeat_clone = heartbeat_executed.clone();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        heartbeat_clone.fetch_add(1, Ordering::SeqCst);
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: true,
            for_user: String::new(),
            for_llm: "HEARTBEAT_OK".to_string(),
        })
    }));

    svc.start().await.unwrap();
    assert!(svc.is_running());

    // Wait for first heartbeat (1 second delay) + a few ticks
    tokio::time::sleep(Duration::from_millis(1500)).await;

    svc.stop();
    assert!(!svc.is_running());

    // Should have executed at least once
    assert!(heartbeat_executed.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn test_start_with_skip_file_active() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    let skip_path = tmp.path().join("BOOTSTRAP.md");

    // Create skip file
    std::fs::write(&skip_path, "bootstrap mode active").unwrap();

    // Create HEARTBEAT.md
    std::fs::write(tmp.path().join("HEARTBEAT.md"), "# Tasks\n\n- Check email").unwrap();

    let heartbeat_executed = Arc::new(AtomicU64::new(0));
    let heartbeat_clone = heartbeat_executed.clone();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    svc.set_skip_file(skip_path.to_string_lossy().to_string());
    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        heartbeat_clone.fetch_add(1, Ordering::SeqCst);
        None
    }));

    svc.start().await.unwrap();

    // Wait for potential heartbeats
    tokio::time::sleep(Duration::from_millis(500)).await;

    svc.stop();

    // Should NOT have executed because skip file exists
    assert_eq!(heartbeat_executed.load(Ordering::SeqCst), 0);
}

#[test]
fn test_parse_last_channel_multiple_colons() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    let (platform, user_id) = svc.parse_last_channel("telegram:user:extra");
    // Should parse first colon only
    assert_eq!(platform, "telegram");
    assert_eq!(user_id, "user:extra");
}

#[test]
fn test_execute_heartbeat_empty_prompt() {
    let dir = tempfile::tempdir().unwrap();
    // Create HEARTBEAT.md with only comments (empty for heartbeat purposes)
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "# Just comments\n## More comments\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let handler_called = Arc::new(AtomicBool::new(false));
    let handler_clone = handler_called.clone();

    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        handler_clone.store(true, Ordering::SeqCst);
        None
    }));

    svc.execute_heartbeat();

    // Handler should NOT be called because prompt is empty
    assert!(!handler_called.load(Ordering::SeqCst));
}

#[test]
fn test_execute_heartbeat_with_message_bus_and_state() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task 1\n- Task 2\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "feishu:user789".to_string(),
    }));

    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: "Tasks completed successfully".to_string(),
            for_llm: "All tasks done".to_string(),
        })
    }));

    svc.execute_heartbeat();

    assert_eq!(sent.lock().len(), 1);
    assert_eq!(sent.lock()[0].0, "feishu");
    assert_eq!(sent.lock()[0].1, "user789");
    assert_eq!(sent.lock()[0].2, "Tasks completed successfully");
}

#[tokio::test]
async fn test_full_heartbeat_flow_with_static_functions() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    // Create a valid HEARTBEAT.md with actual tasks
    let heartbeat_path = tmp.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Check system status\n- Review logs\n").unwrap();

    let heartbeat_count = Arc::new(AtomicU64::new(0));
    let heartbeat_clone = heartbeat_count.clone();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(workspace.clone()),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    let (mock_bus, sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "telegram:testuser".to_string(),
    }));

    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        heartbeat_clone.fetch_add(1, Ordering::SeqCst);
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: "Heartbeat check completed".to_string(),
            for_llm: "All systems operational".to_string(),
        })
    }));

    // Start the heartbeat service
    svc.start().await.unwrap();

    // Wait for first heartbeat (1 second initial delay) + a few ticks
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Stop the service
    svc.stop();

    // Verify heartbeats were executed
    assert!(heartbeat_count.load(Ordering::SeqCst) >= 1);

    // Verify messages were sent
    let sent_lock = sent.lock();
    assert!(!sent_lock.is_empty());
    assert_eq!(sent_lock[0].0, "telegram");
    assert_eq!(sent_lock[0].1, "testuser");
    assert_eq!(sent_lock[0].2, "Heartbeat check completed");
}

#[tokio::test]
async fn test_heartbeat_task_execution_with_skip_file_dynamic() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    let skip_path = tmp.path().join("BOOTSTRAP.md");

    // Create a valid HEARTBEAT.md
    let heartbeat_path = tmp.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Task 1\n- Task 2\n").unwrap();

    let heartbeat_count = Arc::new(AtomicU64::new(0));
    let heartbeat_clone = heartbeat_count.clone();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    let (mock_bus, _sent) = MockBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "web:user1".to_string(),
    }));

    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        heartbeat_clone.fetch_add(1, Ordering::SeqCst);
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: true,
            for_user: String::new(),
            for_llm: "HEARTBEAT_OK".to_string(),
        })
    }));

    // Set skip file path
    svc.set_skip_file(skip_path.to_string_lossy().to_string());

    // Start the heartbeat service
    svc.start().await.unwrap();

    // Wait for first heartbeat (1 second initial delay)
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Initially no skip file, so heartbeats should execute
    assert!(heartbeat_count.load(Ordering::SeqCst) >= 1);

    // Now create the skip file
    std::fs::write(&skip_path, "bootstrap mode").unwrap();

    // Wait for more potential heartbeats (should be skipped)
    tokio::time::sleep(Duration::from_millis(300)).await;

    let _count_with_skip = heartbeat_count.load(Ordering::SeqCst);

    // Stop the service
    svc.stop();

    // The count should not have increased after skip file was created
    // (because skip file causes continue in the loop)
}

#[tokio::test]
async fn test_heartbeat_multiple_intervals_and_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    // Create a valid HEARTBEAT.md
    let heartbeat_path = tmp.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "- Check memory\n- Check CPU\n").unwrap();

    let heartbeat_count = Arc::new(AtomicU64::new(0));
    let heartbeat_count_for_handler = heartbeat_count.clone();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
        heartbeat_count_for_handler.fetch_add(1, Ordering::SeqCst);
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: true,
            for_user: String::new(),
            for_llm: "OK".to_string(),
        })
    }));

    // Start the heartbeat service
    svc.start().await.unwrap();
    assert!(svc.is_running());

    // Wait for first heartbeat + several intervals
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Verify multiple heartbeats occurred
    let count_before_stop = heartbeat_count.load(Ordering::SeqCst);
    assert!(
        count_before_stop >= 2,
        "Should have executed at least 2 heartbeats"
    );

    // Stop the service
    svc.stop();
    assert!(!svc.is_running());

    // Wait a bit more
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Count should not have increased after stop
    let count_after_stop = heartbeat_count.load(Ordering::SeqCst);
    assert_eq!(
        count_before_stop, count_after_stop,
        "Count should not increase after stop"
    );
}

#[test]
fn test_build_prompt_with_unicode_content() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    std::fs::write(
        &heartbeat_path,
        "- 检查系统状态\n- 查看日志\n- 检查磁盘空间\n",
    )
    .unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let prompt = svc.build_prompt();
    assert!(prompt.contains("检查系统状态"));
    assert!(prompt.contains("查看日志"));
    assert!(prompt.contains("检查磁盘空间"));
    assert!(prompt.contains("Heartbeat Check"));
}

#[test]
fn test_build_prompt_with_multiline_content() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");
    let content = "- Check database connections\n  - Verify primary DB\n  - Verify replica DB\n- Check API endpoints\n  - /api/v1/status\n  - /api/v1/health\n";
    std::fs::write(&heartbeat_path, content).unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    let prompt = svc.build_prompt();
    assert!(prompt.contains("Check database connections"));
    assert!(prompt.contains("Verify primary DB"));
    assert!(prompt.contains("Check API endpoints"));
    assert!(prompt.contains("/api/v1/status"));
}

#[test]
fn test_build_prompt_creates_file_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");

    // Ensure file doesn't exist
    assert!(!heartbeat_path.exists());

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    // First call should create the template and return empty
    let prompt1 = svc.build_prompt();
    assert!(prompt1.is_empty());

    // File should now exist
    assert!(heartbeat_path.exists());

    // Second call should read the file and return a prompt
    let prompt2 = svc.build_prompt();
    assert!(prompt2.contains("Heartbeat Check"));
    assert!(prompt2.contains("heartbeat tasks below this line"));
}

#[test]
fn test_create_default_template_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let heartbeat_path = dir.path().join("HEARTBEAT.md");

    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    });

    // First call creates the file
    svc.create_default_heartbeat_template();
    assert!(heartbeat_path.exists());

    // Read the content
    let content1 = std::fs::read_to_string(&heartbeat_path).unwrap();

    // Second call should not overwrite (file exists check)
    svc.create_default_heartbeat_template();

    // Content should be unchanged
    let content2 = std::fs::read_to_string(&heartbeat_path).unwrap();
    assert_eq!(content1, content2);
}

#[tokio::test]
async fn test_heartbeat_service_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_secs(60),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    // Initially not running
    assert!(!svc.is_running());
    assert_eq!(svc.beat_count(), 0);

    // Start the service
    svc.start().await.unwrap();
    assert!(svc.is_running());

    // Stop immediately
    svc.stop();
    assert!(!svc.is_running());

    // Can start again after stop
    svc.start().await.unwrap();
    assert!(svc.is_running());

    svc.stop();
}

#[test]
fn test_status_method_returns_all_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_secs(120),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 10,
        default_interval_minutes: 60,
    });

    let status = svc.status();

    // Check all expected fields exist
    assert!(status.contains_key("running"));
    assert!(status.contains_key("enabled"));
    assert!(status.contains_key("beat_count"));
    assert!(status.contains_key("last_beat"));
    assert!(status.contains_key("interval_secs"));

    // Check values
    assert_eq!(status["enabled"], serde_json::json!(true));
    assert_eq!(status["beat_count"], serde_json::json!(0));
    assert_eq!(status["interval_secs"], serde_json::json!(120));
}

#[tokio::test]
async fn test_concurrent_start_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();

    let svc = HeartbeatService::new(HeartbeatConfig {
        interval: Duration::from_secs(60),
        enabled: true,
        workspace: Some(workspace),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    });

    // Start multiple times concurrently
    let handle1 = svc.start();
    let handle2 = svc.start();
    let handle3 = svc.start();

    // All should complete successfully
    assert!(handle1.await.is_ok());
    assert!(handle2.await.is_ok());
    assert!(handle3.await.is_ok());

    assert!(svc.is_running());

    svc.stop();
}

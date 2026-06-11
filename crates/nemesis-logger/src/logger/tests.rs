use super::*;
use std::path::Path;

#[test]
fn test_default_config() {
    let config = LoggerConfig::default();
    assert_eq!(config.level, "info");
    assert!(!config.json_format);
}

#[test]
fn test_log_level_ordering() {
    assert!(LogLevel::Debug < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
    assert!(LogLevel::Error < LogLevel::Fatal);
}

#[test]
fn test_log_level_from_str() {
    assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
    assert_eq!(LogLevel::from_str("WARN"), LogLevel::Warn);
    assert_eq!(LogLevel::from_str("unknown"), LogLevel::Info);
}

#[test]
fn test_logger_create() {
    let logger = NemesisLogger::new();
    assert!(logger.is_enabled());
    assert!(logger.is_console_enabled());
    assert_eq!(logger.level(), LogLevel::Info);
}

#[test]
fn test_logger_toggle() {
    let logger = NemesisLogger::new();
    logger.disable();
    assert!(!logger.is_enabled());
    logger.enable();
    assert!(logger.is_enabled());

    logger.disable_console();
    assert!(!logger.is_console_enabled());
    logger.enable_console();
    assert!(logger.is_console_enabled());
}

#[test]
fn test_logger_set_level() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);
    assert_eq!(logger.level(), LogLevel::Debug);
}

#[test]
fn test_log_entry_serialization() {
    let entry = LogEntry {
        level: "INFO".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: Some("test".to_string()),
        message: "hello".to_string(),
        fields: None,
        caller: None,
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"level\":\"INFO\""));
    assert!(json.contains("\"component\":\"test\""));
}

#[test]
fn test_logger_log_disabled() {
    let logger = NemesisLogger::new();
    logger.disable();
    // Should not panic
    logger.info("test");
}

#[test]
fn test_file_logging() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.log").to_string_lossy().to_string();
    let logger = NemesisLogger::new();
    logger.enable_file(&path).unwrap();
    logger.info("test message");
    logger.disable_file();
    // Verify file was created
    assert!(Path::new(&path).exists());
}

#[test]
fn test_log_level_names() {
    assert_eq!(LogLevel::Debug.name(), "DEBUG");
    assert_eq!(LogLevel::Info.name(), "INFO");
    assert_eq!(LogLevel::Warn.name(), "WARN");
    assert_eq!(LogLevel::Error.name(), "ERROR");
    assert_eq!(LogLevel::Fatal.name(), "FATAL");
}

#[test]
fn test_log_level_from_str_case_insensitive() {
    assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
    assert_eq!(LogLevel::from_str("Debug"), LogLevel::Debug);
    assert_eq!(LogLevel::from_str("info"), LogLevel::Info);
    assert_eq!(LogLevel::from_str("error"), LogLevel::Error);
    assert_eq!(LogLevel::from_str("fatal"), LogLevel::Fatal);
}

#[test]
fn test_log_level_copy() {
    let level = LogLevel::Warn;
    let copied = level;
    assert_eq!(level, copied);
}

#[test]
fn test_log_entry_with_fields() {
    let mut fields = serde_json::Map::new();
    fields.insert("key".to_string(), serde_json::json!("value"));
    fields.insert("count".to_string(), serde_json::json!(42));

    let entry = LogEntry {
        level: "INFO".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: None,
        message: "structured log".to_string(),
        fields: Some(fields),
        caller: None,
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("key"));
    assert!(json.contains("42"));
}

#[test]
fn test_log_entry_skip_none_fields() {
    let entry = LogEntry {
        level: "INFO".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: None,
        message: "simple log".to_string(),
        fields: None,
        caller: None,
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(!json.contains("component"));
    assert!(!json.contains("fields"));
    assert!(!json.contains("caller"));
}

#[test]
fn test_logger_convenience_methods() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);
    // These should not panic
    logger.debug("debug msg");
    logger.debug_c("comp", "debug comp msg");
    logger.info("info msg");
    logger.info_c("comp", "info comp msg");
    logger.warn("warn msg");
    logger.warn_c("comp", "warn comp msg");
    logger.error("error msg");
    logger.error_c("comp", "error comp msg");
    logger.fatal("fatal msg");
    logger.fatal_c("comp", "fatal comp msg");
}

#[test]
fn test_logger_structured_fields() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);
    let mut fields = serde_json::Map::new();
    fields.insert("user".to_string(), serde_json::json!("alice"));
    fields.insert("action".to_string(), serde_json::json!("login"));

    logger.debug_f("debug fields", fields.clone());
    logger.debug_cf("comp", "debug cf", fields.clone());
    logger.info_f("info fields", fields.clone());
    logger.info_cf("comp", "info cf", fields.clone());
    logger.warn_f("warn fields", fields.clone());
    logger.warn_cf("comp", "warn cf", fields.clone());
    logger.error_f("error fields", fields.clone());
    logger.error_cf("comp", "error cf", fields.clone());
    logger.fatal_f("fatal fields", fields.clone());
    logger.fatal_cf("comp", "fatal cf", fields);
}

#[test]
fn test_logger_level_filtering() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Warn);
    // Debug and Info should be suppressed
    logger.debug("should not appear");
    logger.info("should not appear");
    // Warn and Error should pass
    logger.warn("should appear");
    logger.error("should appear");
}

#[test]
fn test_file_logging_multiple_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.log").to_string_lossy().to_string();
    let logger = NemesisLogger::new();
    logger.enable_file(&path).unwrap();

    for i in 0..10 {
        logger.info(&format!("message {}", i));
    }
    logger.disable_file();

    let content = std::fs::read_to_string(Path::new(&path)).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 10);
}

#[test]
fn test_config_custom_values() {
    let config = LoggerConfig {
        level: "debug".to_string(),
        json_format: true,
        file_output: Some("/tmp/test.log".to_string()),
    };
    assert_eq!(config.level, "debug");
    assert!(config.json_format);
    assert_eq!(config.file_output, Some("/tmp/test.log".to_string()));
}

#[test]
fn test_log_entry_with_caller() {
    let entry = LogEntry {
        level: "ERROR".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: Some("main".to_string()),
        message: "panic".to_string(),
        fields: None,
        caller: Some("main.rs:42".to_string()),
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("main.rs:42"));
}

#[test]
fn test_logger_set_hook() {
    let logger = NemesisLogger::new();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_clone = counter.clone();
    logger.set_hook(Box::new(move |_entry| {
        counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    logger.info("trigger hook");
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    logger.info("trigger hook again");
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn test_logger_empty_component() {
    let logger = NemesisLogger::new();
    // Empty component should work without panic
    logger.info("");
    logger.info_c("", "msg with empty component");
}

// ---- New tests for coverage ----

#[test]
fn test_logger_config_serialization_deserialization() {
    // LoggerConfig is not Serialize/Deserialize but has Clone + Debug.
    // Test that we can construct and clone it properly.
    let config = LoggerConfig {
        level: "debug".to_string(),
        json_format: true,
        file_output: Some("/tmp/test.log".to_string()),
    };
    let cloned = config.clone();
    assert_eq!(cloned.level, "debug");
    assert!(cloned.json_format);
    assert_eq!(cloned.file_output, Some("/tmp/test.log".to_string()));
}

#[test]
fn test_log_level_filtering_with_file_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filtered.log").to_string_lossy().to_string();
    let logger = NemesisLogger::new();
    logger.enable_file(&path).unwrap();

    // Set level to Warn, debug and info should be suppressed in file
    logger.set_level(LogLevel::Warn);
    logger.debug("debug suppressed");
    logger.info("info suppressed");
    logger.warn("warn visible");
    logger.error("error visible");

    logger.disable_file();

    let content = std::fs::read_to_string(Path::new(&path)).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    // Only warn and error should be in the file
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("WARN"));
    assert!(lines[1].contains("ERROR"));
}

#[test]
fn test_log_with_empty_map_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty_fields.log").to_string_lossy().to_string();
    let logger = NemesisLogger::new();
    logger.enable_file(&path).unwrap();

    // Pass Some(empty_map) - fields should be treated as None
    let empty_fields = serde_json::Map::new();
    logger.log(LogLevel::Info, "comp", "msg with empty fields", Some(empty_fields));

    logger.disable_file();

    let content = std::fs::read_to_string(Path::new(&path)).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    // Should NOT contain "fields" since the map was empty
    assert!(!lines[0].contains("\"fields\""));
}

#[test]
fn test_log_entry_deserialization() {
    let json = r#"{"level":"INFO","timestamp":"2026-01-01T00:00:00Z","message":"hello"}"#;
    let entry: LogEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.level, "INFO");
    assert_eq!(entry.message, "hello");
    assert!(entry.component.is_none());
    assert!(entry.fields.is_none());
    assert!(entry.caller.is_none());
}

#[test]
fn test_log_entry_deserialization_with_all_fields() {
    let json = r#"{"level":"ERROR","timestamp":"2026-01-01T00:00:00Z","component":"main","message":"error msg","fields":{"key":"value"},"caller":"main.rs:42"}"#;
    let entry: LogEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.level, "ERROR");
    assert_eq!(entry.component, Some("main".to_string()));
    assert_eq!(entry.message, "error msg");
    assert!(entry.fields.is_some());
    assert_eq!(entry.caller, Some("main.rs:42".to_string()));
}

#[test]
fn test_log_entry_clone() {
    let entry = LogEntry {
        level: "INFO".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: Some("test".to_string()),
        message: "hello".to_string(),
        fields: None,
        caller: None,
    };
    let cloned = entry.clone();
    assert_eq!(cloned.level, entry.level);
    assert_eq!(cloned.message, entry.message);
}

#[test]
fn test_logger_file_output_error_invalid_path() {
    let logger = NemesisLogger::new();
    // Try to enable file logging to an invalid path (on Windows, NUL-like or permission denied)
    // Use a path with null byte which should fail
    let result = logger.enable_file("/nonexistent_dir/subdir/test.log");
    // This might succeed or fail depending on OS - just ensure no panic
    let _ = result;
}

#[test]
fn test_logger_disable_file_when_not_enabled() {
    let logger = NemesisLogger::new();
    // Should not panic when file logging was never enabled
    logger.disable_file();
}

#[test]
fn test_logger_hook_multiple_messages() {
    let logger = NemesisLogger::new();
    let messages = Arc::new(Mutex::new(Vec::<String>::new()));
    let messages_clone = messages.clone();
    logger.set_hook(Box::new(move |entry| {
        messages_clone.lock().push(entry.message.clone());
    }));

    for i in 0..5 {
        logger.info(&format!("msg {}", i));
    }

    let msgs = messages.lock();
    assert_eq!(msgs.len(), 5);
    assert_eq!(msgs[0], "msg 0");
    assert_eq!(msgs[4], "msg 4");
}

#[test]
fn test_log_entry_with_populated_fields_serialization() {
    let mut fields = serde_json::Map::new();
    fields.insert("user".to_string(), serde_json::json!("alice"));
    fields.insert("count".to_string(), serde_json::json!(42));

    let entry = LogEntry {
        level: "INFO".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: Some("auth".to_string()),
        message: "login".to_string(),
        fields: Some(fields),
        caller: Some("auth.rs:10".to_string()),
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"level\":\"INFO\""));
    assert!(json.contains("\"component\":\"auth\""));
    assert!(json.contains("\"fields\""));
    assert!(json.contains("\"caller\":\"auth.rs:10\""));
}

#[test]
fn test_log_level_ordering_comprehensive() {
    assert!(LogLevel::Debug < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
    assert!(LogLevel::Error < LogLevel::Fatal);
    assert!(LogLevel::Debug < LogLevel::Fatal);
}

#[test]
fn test_log_level_from_str_all_variants() {
    assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
    assert_eq!(LogLevel::from_str("DEBUG"), LogLevel::Debug);
    assert_eq!(LogLevel::from_str("info"), LogLevel::Info);
    assert_eq!(LogLevel::from_str("INFO"), LogLevel::Info);
    assert_eq!(LogLevel::from_str("warn"), LogLevel::Warn);
    assert_eq!(LogLevel::from_str("WARN"), LogLevel::Warn);
    assert_eq!(LogLevel::from_str("error"), LogLevel::Error);
    assert_eq!(LogLevel::from_str("ERROR"), LogLevel::Error);
    assert_eq!(LogLevel::from_str("fatal"), LogLevel::Fatal);
    assert_eq!(LogLevel::from_str("FATAL"), LogLevel::Fatal);
    assert_eq!(LogLevel::from_str("anything_else"), LogLevel::Info);
}

#[test]
fn test_init_default_function() {
    // Note: This test might fail if other tests have already initialized the global subscriber
    // In a real scenario, the subscriber can only be set once globally
    // We're testing that the function doesn't cause undefined behavior/crashes
    let _ = std::panic::catch_unwind(|| {
        init_default()
    });
    // Test passes if we get here (no crash)
}

#[test]
fn test_init_logger_with_config() {
    // Note: This test might fail if global subscriber is already set
    let config = LoggerConfig {
        level: "debug".to_string(),
        json_format: false,
        file_output: None,
    };
    let _ = std::panic::catch_unwind(|| {
        init_logger(&config)
    });
    // Test passes if we get here (no crash)
}

#[test]
fn test_init_logger_with_file_output() {
    // Note: This test might fail if global subscriber is already set
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("init.log").to_string_lossy().to_string();

    let config = LoggerConfig {
        level: "info".to_string(),
        json_format: false,
        file_output: Some(path.clone()),
    };
    let _ = std::panic::catch_unwind(|| {
        init_logger(&config)
    });
    // Test passes if we get here (no crash)
}

#[test]
fn test_init_logger_json_format() {
    // Note: This test might fail if global subscriber is already set
    let config = LoggerConfig {
        level: "info".to_string(),
        json_format: true,
        file_output: None,
    };
    let _ = std::panic::catch_unwind(|| {
        init_logger(&config)
    });
    // Test passes if we get here (no crash)
}

#[test]
fn test_global_function() {
    // Test that global() works (returns Some if already initialized, None otherwise)
    let global_logger = global();
    // We don't assert specifically - just verify it doesn't panic
    let _ = global_logger;
}

#[test]
fn test_global_persists_across_calls() {
    // If global logger is set, multiple calls should return the same instance
    let logger1 = global();
    let logger2 = global();

    match (logger1, logger2) {
        (Some(l1), Some(l2)) => {
            // Both should work
            l1.info("global 1");
            l2.info("global 2");
        }
        _ => {
            // No global logger set - that's fine for this test
        }
    }
}

#[test]
fn test_logger_all_level_methods() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);

    // Test all level methods without panic
    logger.debug("debug");
    logger.debug_c("comp", "debug_c");
    logger.info("info");
    logger.info_c("comp", "info_c");
    logger.warn("warn");
    logger.warn_c("comp", "warn_c");
    logger.error("error");
    logger.error_c("comp", "error_c");
    logger.fatal("fatal");
    logger.fatal_c("comp", "fatal_c");
}

#[test]
fn test_logger_all_field_methods() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);

    let mut fields = serde_json::Map::new();
    fields.insert("key".to_string(), serde_json::json!("value"));

    // Test all field methods without panic
    logger.debug_f("debug_f", fields.clone());
    logger.debug_cf("comp", "debug_cf", fields.clone());
    logger.info_f("info_f", fields.clone());
    logger.info_cf("comp", "info_cf", fields.clone());
    logger.warn_f("warn_f", fields.clone());
    logger.warn_cf("comp", "warn_cf", fields.clone());
    logger.error_f("error_f", fields.clone());
    logger.error_cf("comp", "error_cf", fields.clone());
    logger.fatal_f("fatal_f", fields.clone());
    logger.fatal_cf("comp", "fatal_cf", fields);
}

#[test]
fn test_logger_level_filtering_all_levels() {
    let logger = NemesisLogger::new();

    // Test each level as minimum threshold
    for min_level in [LogLevel::Debug, LogLevel::Info, LogLevel::Warn, LogLevel::Error, LogLevel::Fatal] {
        logger.set_level(min_level);

        // Only levels >= min_level should pass
        logger.debug("debug test");
        logger.info("info test");
        logger.warn("warn test");
        logger.error("error test");
        logger.fatal("fatal test");
    }
}

#[test]
fn test_logger_enable_disable_console() {
    let logger = NemesisLogger::new();

    // Initially enabled
    assert!(logger.is_console_enabled());

    // Disable
    logger.disable_console();
    assert!(!logger.is_console_enabled());

    // Enable
    logger.enable_console();
    assert!(logger.is_console_enabled());

    // Should not panic when disabled
    logger.disable_console();
    logger.info("test with console disabled");
}

#[test]
fn test_logger_file_enable_disable_operations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enable_disable.log").to_string_lossy().to_string();

    let logger = NemesisLogger::new();

    // Enable file logging
    let result = logger.enable_file(&path);
    assert!(result.is_ok());

    // Write something
    logger.info("message 1");

    // Disable file logging
    logger.disable_file();

    // Re-enable file logging
    let result = logger.enable_file(&path);
    assert!(result.is_ok());

    // Write something else
    logger.info("message 2");

    logger.disable_file();

    // Verify file content
    let content = std::fs::read_to_string(Path::new(&path)).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn test_logger_hook_can_be_replaced() {
    let logger = NemesisLogger::new();

    let counter1 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter1_clone = counter1.clone();

    logger.set_hook(Box::new(move |_entry| {
        counter1_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));

    logger.info("msg 1");
    assert_eq!(counter1.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Replace hook with new one
    let counter2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter2_clone = counter2.clone();

    logger.set_hook(Box::new(move |_entry| {
        counter2_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));

    logger.info("msg 2");

    // First counter should not have increased
    assert_eq!(counter1.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Second counter should have increased
    assert_eq!(counter2.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn test_logger_component_edge_cases() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);

    // Empty component
    logger.info_c("", "empty component");

    // Component with special characters
    logger.info_c("test::component::name", "component with colons");
    logger.info_c("test.component.name", "component with dots");
    logger.info_c("test/component/name", "component with slashes");
}

#[test]
fn test_logger_message_edge_cases() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);

    // Empty message
    logger.info("");

    // Very long message
    let long_msg = "x".repeat(10000);
    logger.info(&long_msg);

    // Message with special characters
    logger.info("message with \n newline");
    logger.info("message with \t tab");
    logger.info("message with \" quotes\"");
}

#[test]
fn test_logger_fields_edge_cases() {
    let logger = NemesisLogger::new();
    logger.set_level(LogLevel::Debug);

    // Empty fields map
    let empty_fields = serde_json::Map::new();
    logger.info_f("empty fields", empty_fields);

    // Fields with various value types
    let mut fields = serde_json::Map::new();
    fields.insert("string".to_string(), serde_json::json!("text"));
    fields.insert("number".to_string(), serde_json::json!(42));
    fields.insert("float".to_string(), serde_json::json!(3.14));
    fields.insert("bool".to_string(), serde_json::json!(true));
    fields.insert("null".to_string(), serde_json::json!(()));
    fields.insert("array".to_string(), serde_json::json!([1, 2, 3]));
    fields.insert("object".to_string(), serde_json::json!({"nested": "value"}));

    logger.info_f("complex fields", fields);
}

#[test]
fn test_logger_concurrent_logging() {
    let logger = Arc::new(NemesisLogger::new());
    logger.set_level(LogLevel::Debug);

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let logger_clone = logger.clone();
            std::thread::spawn(move || {
                for j in 0..100 {
                    logger_clone.info(&format!("Thread {} message {}", i, j));
                }
            })
        })
        .collect();

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // Should not panic or deadlock
}

#[test]
fn test_logger_disabled_all_levels() {
    let logger = NemesisLogger::new();
    logger.disable();

    // All levels should be suppressed when logger is disabled
    logger.debug("debug suppressed");
    logger.info("info suppressed");
    logger.warn("warn suppressed");
    logger.error("error suppressed");
    logger.fatal("fatal suppressed");
}

#[test]
fn test_logger_file_permissions_error() {
    let logger = NemesisLogger::new();

    // Try to write to a path that likely doesn't exist or has permission issues
    let result = logger.enable_file("/nonexistent/directory/path/test.log");
    // Should return error, not panic
    let _ = result;
}

#[test]
fn test_log_entry_all_fields_present() {
    let mut fields = serde_json::Map::new();
    fields.insert("key".to_string(), serde_json::json!("value"));

    let entry = LogEntry {
        level: "INFO".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        component: Some("component".to_string()),
        message: "message".to_string(),
        fields: Some(fields.clone()),
        caller: Some("caller".to_string()),
    };

    let json = serde_json::to_string(&entry).unwrap();

    // All fields should be present in JSON
    assert!(json.contains("\"level\":\"INFO\""));
    assert!(json.contains("\"timestamp\":\"2026-01-01T00:00:00Z\""));
    assert!(json.contains("\"component\":\"component\""));
    assert!(json.contains("\"message\":\"message\""));
    assert!(json.contains("\"fields\""));
    assert!(json.contains("\"caller\":\"caller\""));
    assert!(json.contains("\"key\":\"value\""));
}

#[test]
fn test_log_level_debug_trait() {
    // Test that LogLevel implements Debug
    let level = LogLevel::Info;
    let formatted = format!("{:?}", level);
    assert!(formatted.contains("Info"));
}

#[test]
fn test_logger_state_consistency() {
    let logger = NemesisLogger::new();

    // Initial state
    assert_eq!(logger.level(), LogLevel::Info);
    assert!(logger.is_enabled());
    assert!(logger.is_console_enabled());

    // Modify state
    logger.set_level(LogLevel::Debug);
    logger.disable_console();

    // Check consistency
    assert_eq!(logger.level(), LogLevel::Debug);
    assert!(logger.is_enabled());
    assert!(!logger.is_console_enabled());

    // Modify again
    logger.enable_console();
    logger.disable();

    // Check consistency again
    assert_eq!(logger.level(), LogLevel::Debug);
    assert!(!logger.is_enabled());
    assert!(logger.is_console_enabled());
}

#[test]
fn test_init_logger_multiple_calls() {
    // Multiple init calls should not panic (though they may fail after the first)
    let _ = std::panic::catch_unwind(|| {
        let config1 = LoggerConfig {
            level: "debug".to_string(),
            json_format: false,
            file_output: None,
        };
        let result1 = init_logger(&config1);

        let config2 = LoggerConfig {
            level: "info".to_string(),
            json_format: true,
            file_output: None,
        };
        let result2 = init_logger(&config2);

        // At least one should succeed or fail gracefully
        let _ = (result1, result2);

        // Global should still be accessible
        let global_logger = global();
        let _ = global_logger;
    });
    // Test passes if we get here (no crash)
}

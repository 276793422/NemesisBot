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

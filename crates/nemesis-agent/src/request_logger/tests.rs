use super::*;
use tempfile::TempDir;

fn test_config() -> LoggingConfig {
    LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Full,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    }
}

#[test]
fn disabled_logger_is_noop() {
    let config = LoggingConfig {
        enabled: false,
        detail_level: DetailLevel::Full,
        log_dir: String::new(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(config, tmp.path());

    assert!(!logger.is_enabled());
    logger.log_user_request(&UserRequestInfo {
        timestamp: Utc::now(),
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
    });
    // No crash, no files created.
}

#[test]
fn create_session_and_log_user_request() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    assert!(logger.is_enabled());

    logger.create_session().unwrap();
    let session_dir = logger.session_dir().unwrap();
    assert!(session_dir.exists());

    logger.log_user_request(&UserRequestInfo {
        timestamp: Utc::now(),
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Test message".to_string(),
    });

    // Find the request file
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].file_name().to_string_lossy().ends_with(".request.md"));

    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("# User Request"));
    assert!(content.contains("Test message"));
    assert!(content.contains("web"));
}

#[test]
fn log_llm_request_and_response() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_llm_request(&LLMRequestInfo {
        round: 1,
        timestamp: Utc::now(),
        model: "gpt-4".to_string(),
        provider_name: "openai".to_string(),
        api_key: "sk-1234567890abcdef".to_string(),
        api_base: "https://api.openai.com".to_string(),
        messages_count: 5,
        tools_count: 3,
        messages: Vec::new(),
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });

    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Utc::now(),
        duration_ms: 1500,
        content: "The answer is 42.".to_string(),
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo::default(),
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 2);

    // Check request file
    let req_content = fs::read_to_string(
        entries.iter().find(|e| e.file_name().to_string_lossy().contains("Request")).unwrap().path(),
    )
    .unwrap();
    assert!(req_content.contains("gpt-4"));
    assert!(req_content.contains("sk-***def")); // Masked API key

    // Check response file
    let resp_content = fs::read_to_string(
        entries.iter().find(|e| e.file_name().to_string_lossy().contains("Response")).unwrap().path(),
    )
    .unwrap();
    assert!(resp_content.contains("The answer is 42."));
    assert!(resp_content.contains("stop"));
}

#[test]
fn log_local_operations() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_local_operations(&LocalOperationInfo {
        round: 1,
        timestamp: Utc::now(),
        operations: vec![
            OperationInfo {
                op_type: "tool_call".to_string(),
                name: "calculator".to_string(),
                status: "Success".to_string(),
                error: String::new(),
                duration_ms: 50,
                arguments: r#"{"expr":"2+2"}"#.to_string(),
                result: "4".to_string(),
            },
            OperationInfo {
                op_type: "file_read".to_string(),
                name: "read_config".to_string(),
                status: "Failed".to_string(),
                error: "file not found".to_string(),
                duration_ms: 10,
                arguments: String::new(),
                result: String::new(),
            },
        ],
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);

    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("Tool Execution"));
    assert!(content.contains("calculator"));
    assert!(content.contains("Failed"));
    assert!(content.contains("file not found"));
}

#[test]
fn log_final_response() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_final_response(&FinalResponseInfo {
        timestamp: Utc::now(),
        total_duration_ms: 3500,
        llm_rounds: 3,
        content: "Final answer here.".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);

    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("Final answer here."));
    assert!(content.contains("3.5s"));
    assert!(content.contains("LLM Rounds**: 3"));
}

#[test]
fn log_local_operations_skips_empty() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_local_operations(&LocalOperationInfo {
        round: 1,
        timestamp: Utc::now(),
        operations: vec![],
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 0);
}

#[test]
fn mask_api_key_tests() {
    assert_eq!(mask_api_key(""), "<empty>");
    assert_eq!(mask_api_key("short"), "***");
    assert_eq!(mask_api_key("sk-1234567890abcdef"), "sk-***def");
    assert_eq!(mask_api_key("  sk-1234567890abcdef  "), "sk-***def");
}

#[test]
fn format_operation_type_tests() {
    assert_eq!(format_operation_type("tool_call"), "Tool Execution");
    assert_eq!(format_operation_type("file_write"), "File Write");
    assert_eq!(format_operation_type("file_read"), "File Read");
    assert_eq!(format_operation_type("command_exec"), "Command Execution");
    assert_eq!(format_operation_type("custom_op"), "custom op");
}

#[test]
fn truncated_mode_truncates_long_content() {
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Truncated,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(config, tmp.path());
    logger.create_session().unwrap();

    let long_content = "x".repeat(1000);
    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Utc::now(),
        duration_ms: 100,
        content: long_content,
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo::default(),
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);

    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("truncated"));
}

#[test]
fn resolve_log_path_absolute() {
    let workspace = Path::new("/workspace");
    let result = resolve_log_path("/var/log", workspace);
    assert_eq!(result, PathBuf::from("/var/log"));
}

#[test]
fn resolve_log_path_relative() {
    let workspace = Path::new("/workspace");
    let result = resolve_log_path("logs/llm", workspace);
    assert_eq!(result, PathBuf::from("/workspace/logs/llm"));
}

#[test]
fn log_llm_request_with_tool_calls() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Utc::now(),
        duration_ms: 500,
        content: "Using tools".to_string(),
        tool_calls_count: 2,
        finish_reason: "tool_calls".to_string(),
        tool_calls: vec![
            ToolCallDetail {
                id: "tc-1".to_string(),
                name: "file_read".to_string(),
                arguments: r#"{"path": "/etc/config"}"#.to_string(),
            },
            ToolCallDetail {
                id: "tc-2".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr": "2+2"}"#.to_string(),
            },
        ],
        usage: UsageInfo {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cached_tokens: 0,
        },
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);

    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("file_read"));
    assert!(content.contains("calculator"));
    assert!(content.contains("tc-1"));
    assert!(content.contains("150"));
}

#[test]
fn log_llm_request_with_fallback_attempts() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_llm_request(&LLMRequestInfo {
        round: 1,
        timestamp: Utc::now(),
        model: "gpt-4".to_string(),
        provider_name: "openai".to_string(),
        api_key: "sk-test123456789".to_string(),
        api_base: "https://api.openai.com".to_string(),
        messages_count: 3,
        tools_count: 5,
        messages: Vec::new(),
        http_headers: vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), "Bearer sk-test123456789".to_string()),
        ],
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("temperature".to_string(), "0.7".to_string());
            m
        },
        fallback_attempts: vec![
            FallbackAttemptInfo {
                provider: "openai".to_string(),
                model: "gpt-4".to_string(),
                api_key: "sk-test123456789".to_string(),
                api_base: "https://api.openai.com".to_string(),
                error: "rate limited".to_string(),
                duration_ms: 5000,
            },
        ],
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);

    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("Fallback Attempts"));
    assert!(content.contains("rate limited"));
    assert!(content.contains("application/json"));
    assert!(content.contains("temperature"));
}

#[test]
fn disabled_logger_all_methods_noop() {
    let config = LoggingConfig {
        enabled: false,
        detail_level: DetailLevel::Full,
        log_dir: String::new(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(config, tmp.path());

    assert!(!logger.is_enabled());

    // None of these should panic
    logger.create_session().unwrap();
    logger.log_user_request(&UserRequestInfo {
        timestamp: Utc::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "test".to_string(),
    });
    logger.log_llm_request(&LLMRequestInfo::default());
    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Utc::now(),
        duration_ms: 100,
        content: "test".to_string(),
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo::default(),
    });
    logger.log_local_operations(&LocalOperationInfo {
        round: 1,
        timestamp: Utc::now(),
        operations: vec![OperationInfo::default()],
    });
    logger.log_final_response(&FinalResponseInfo {
        timestamp: Utc::now(),
        total_duration_ms: 100,
        llm_rounds: 1,
        content: "test".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
    });

    assert!(logger.session_dir().is_none());
}

#[test]
fn multiple_sessions() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());

    // First session
    logger.create_session().unwrap();
    logger.log_user_request(&UserRequestInfo {
        timestamp: Utc::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "first".to_string(),
    });

    // Second session (overwrites first)
    logger.create_session().unwrap();
    logger.log_user_request(&UserRequestInfo {
        timestamp: Utc::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "second".to_string(),
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("second"));
}

#[test]
fn log_with_special_characters() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_user_request(&UserRequestInfo {
        timestamp: Utc::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "Hello <script>alert('xss')</script> & 'quotes' \"double\"".to_string(),
    });

    let session_dir = logger.session_dir().unwrap();
    let entries: Vec<_> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("script"));
}

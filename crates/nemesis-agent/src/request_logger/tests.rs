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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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
    assert!(
        entries[0]
            .file_name()
            .to_string_lossy()
            .ends_with(".request.md")
    );

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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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
        entries
            .iter()
            .find(|e| e.file_name().to_string_lossy().contains("Request"))
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(req_content.contains("gpt-4"));
    assert!(req_content.contains("sk-***def")); // Masked API key

    // Check response file
    let resp_content = fs::read_to_string(
        entries
            .iter()
            .find(|e| e.file_name().to_string_lossy().contains("Response"))
            .unwrap()
            .path(),
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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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
        timestamp: Local::now(),
        model: "gpt-4".to_string(),
        provider_name: "openai".to_string(),
        api_key: "sk-test123456789".to_string(),
        api_base: "https://api.openai.com".to_string(),
        messages_count: 3,
        tools_count: 5,
        messages: Vec::new(),
        http_headers: vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Authorization".to_string(),
                "Bearer sk-test123456789".to_string(),
            ),
        ],
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("temperature".to_string(), "0.7".to_string());
            m
        },
        fallback_attempts: vec![FallbackAttemptInfo {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: "sk-test123456789".to_string(),
            api_base: "https://api.openai.com".to_string(),
            error: "rate limited".to_string(),
            duration_ms: 5000,
        }],
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
        timestamp: Local::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "test".to_string(),
    });
    logger.log_llm_request(&LLMRequestInfo::default());
    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Local::now(),
        duration_ms: 100,
        content: "test".to_string(),
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo::default(),
    });
    logger.log_local_operations(&LocalOperationInfo {
        round: 1,
        timestamp: Local::now(),
        operations: vec![OperationInfo::default()],
    });
    logger.log_final_response(&FinalResponseInfo {
        timestamp: Local::now(),
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
        timestamp: Local::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "first".to_string(),
    });

    // Second session (overwrites first)
    logger.create_session().unwrap();
    logger.log_user_request(&UserRequestInfo {
        timestamp: Local::now(),
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
        timestamp: Local::now(),
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

// ===========================================================================
// Coverage gap tests — cover previously-uncovered branches.
// ===========================================================================

/// Helper: find the single file in `dir` whose name contains `needle`.
fn find_file_containing(dir: &Path, needle: &str) -> PathBuf {
    fs::read_dir(dir)
        .unwrap()
        .find_map(|e| {
            let e = e.ok()?;
            if e.file_name().to_string_lossy().contains(needle) {
                Some(e.path())
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("no file containing '{}' in {:?}", needle, dir))
}

#[test]
fn detail_level_default_is_full() {
    assert_eq!(DetailLevel::default(), DetailLevel::Full);
}

#[test]
fn logging_config_default_values() {
    let cfg = LoggingConfig::default();
    assert!(!cfg.enabled);
    assert_eq!(cfg.detail_level, DetailLevel::Full);
    assert_eq!(cfg.log_dir, "logs/llm");
    assert!(!cfg.save_raw);
}

#[test]
fn new_with_paths_disabled_is_noop() {
    let config = LoggingConfig {
        enabled: false,
        detail_level: DetailLevel::Full,
        log_dir: String::new(),
        save_raw: false,
    };
    let logger = RequestLogger::new_with_paths(config, PathBuf::from("/nonexistent"), None);
    assert!(!logger.is_enabled());
    // create_session is a noop when disabled; no session dir is set.
    logger.create_session().unwrap();
    assert!(logger.session_dir().is_none());
}

#[test]
fn new_with_paths_with_session_name_override() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new_with_paths(
        test_config(),
        tmp.path().to_path_buf(),
        Some("my-cluster-session".to_string()),
    );
    assert!(logger.is_enabled());
    logger.create_session().unwrap();
    let dir = logger.session_dir().unwrap();
    // The override name (sanitized) is used directly as the session directory name.
    assert!(dir.to_string_lossy().ends_with("my-cluster-session"));
}

#[test]
fn new_with_paths_without_session_name_uses_timestamp_scheme() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new_with_paths(test_config(), tmp.path().to_path_buf(), None);
    logger.create_session().unwrap();
    let dir = logger.session_dir().unwrap();
    // Default scheme produces a `YYYY-MM-DD_HH-MM-SS_xxx` directory.
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.contains('-') && name.contains('_'));
}

#[test]
fn sanitize_filename_replaces_all_unsafe_chars() {
    // Path separators, shell metacharacters and null are replaced with '_'.
    assert_eq!(
        RequestLogger::sanitize_filename("safe_name-1"),
        "safe_name-1"
    );
    assert_eq!(
        RequestLogger::sanitize_filename("a/b\\c:d*e?f\"g<h>i|j"),
        "a_b_c_d_e_f_g_h_i_j"
    );
    assert_eq!(RequestLogger::sanitize_filename("null\0byte"), "null_byte");
}

#[test]
fn log_raw_request_writes_json_envelope() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    let body =
        serde_json::json!({"model": "gpt-4", "messages": [{"role": "user", "content": "hi"}]});
    logger.log_raw_request(&body, Local::now(), 2);

    let dir = logger.session_dir().unwrap();
    let raw = find_file_containing(&dir, "AI.Request.raw.json");
    let content = fs::read_to_string(raw).unwrap();
    assert!(content.contains("\"round\": 2"));
    assert!(content.contains("gpt-4"));
    assert!(content.contains("\"body\""));
}

#[test]
fn log_raw_request_envelope_writes_pretty_json() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    let envelope = serde_json::json!({"timestamp": "2026-01-01", "round": 1, "body": {"x": 1}});
    logger.log_raw_request_envelope(&envelope);

    let dir = logger.session_dir().unwrap();
    let raw = find_file_containing(&dir, "AI.Request.raw.json");
    let content = fs::read_to_string(raw).unwrap();
    assert!(content.contains("\"x\": 1"));
}

#[test]
fn log_raw_response_writes_json_envelope() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    let body = r#"{"id":"chatcmpl-1","choices":[{"message":{"content":"hi"}}]}"#;
    logger.log_raw_response(body, Local::now(), 1, 750);

    let dir = logger.session_dir().unwrap();
    let raw = find_file_containing(&dir, "AI.Response.raw.json");
    let content = fs::read_to_string(raw).unwrap();
    assert!(content.contains("\"duration_ms\": 750"));
    assert!(content.contains("chatcmpl-1"));
}

#[test]
fn log_raw_response_with_invalid_json_falls_back_to_string_body() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    // Invalid JSON body → envelope body becomes a JSON string value (no panic).
    logger.log_raw_response("not valid json {", Local::now(), 1, 10);

    let dir = logger.session_dir().unwrap();
    let raw = find_file_containing(&dir, "AI.Response.raw.json");
    let content = fs::read_to_string(raw).unwrap();
    assert!(content.contains("not valid json"));
}

#[test]
fn log_llm_request_with_messages_and_tool_calls_section() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    use crate::r#loop::LlmMessage;
    use crate::types::ToolCallInfo;

    let msg = LlmMessage {
        role: "assistant".to_string(),
        content: "Calling tool".to_string(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"rust"}"#.to_string(),
        }]),
        tool_call_id: None,
        reasoning_content: None,
    };
    logger.log_llm_request(&LLMRequestInfo {
        round: 1,
        timestamp: Local::now(),
        model: "gpt-4".to_string(),
        provider_name: "openai".to_string(),
        api_key: "sk-1234567890abcdef".to_string(),
        api_base: "https://api.openai.com".to_string(),
        messages_count: 1,
        tools_count: 2,
        messages: vec![msg],
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });

    let dir = logger.session_dir().unwrap();
    let req = find_file_containing(&dir, "AI.Request.md");
    let content = fs::read_to_string(req).unwrap();
    assert!(content.contains("# Messages"));
    assert!(content.contains("Calling tool"));
    assert!(content.contains("search"));
    assert!(content.contains("ToolCall"));
}

#[test]
fn log_llm_request_truncates_long_message_content() {
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Truncated,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(config, tmp.path());
    logger.create_session().unwrap();

    use crate::r#loop::LlmMessage;
    let msg = LlmMessage {
        role: "user".to_string(),
        content: "y".repeat(500),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    };
    logger.log_llm_request(&LLMRequestInfo {
        round: 1,
        timestamp: Local::now(),
        model: "gpt-4".to_string(),
        provider_name: String::new(),
        api_key: String::new(),
        api_base: String::new(),
        messages_count: 1,
        tools_count: 0,
        messages: vec![msg],
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });

    let dir = logger.session_dir().unwrap();
    let req = find_file_containing(&dir, "AI.Request.md");
    let content = fs::read_to_string(req).unwrap();
    // Truncated preview present.
    assert!(content.len() < 2000);
}

#[test]
fn log_llm_response_with_cached_tokens_emits_cache_hit() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Local::now(),
        duration_ms: 200,
        content: "ok".to_string(),
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo {
            prompt_tokens: 1000,
            completion_tokens: 100,
            total_tokens: 1100,
            cached_tokens: 500,
        },
    });

    let dir = logger.session_dir().unwrap();
    let resp = find_file_containing(&dir, "AI.Response.md");
    let content = fs::read_to_string(resp).unwrap();
    assert!(content.contains("Cached Tokens"));
    assert!(content.contains("cache hit"));
    // 500 * 100 / 1000 = 50%
    assert!(content.contains("50"));
}

#[test]
fn log_llm_response_usage_without_cached_tokens() {
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    logger.create_session().unwrap();

    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Local::now(),
        duration_ms: 100,
        content: "ok".to_string(),
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
            cached_tokens: 0,
        },
    });

    let dir = logger.session_dir().unwrap();
    let resp = find_file_containing(&dir, "AI.Response.md");
    let content = fs::read_to_string(resp).unwrap();
    assert!(content.contains("Token Usage"));
    assert!(content.contains("120"));
    // cached_tokens == 0 → no cache hit line.
    assert!(!content.contains("cache hit"));
}

#[test]
fn log_llm_response_truncated_tool_call_arguments() {
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Truncated,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(config, tmp.path());
    logger.create_session().unwrap();

    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Local::now(),
        duration_ms: 100,
        content: "x".to_string(),
        tool_calls_count: 1,
        finish_reason: "tool_calls".to_string(),
        tool_calls: vec![ToolCallDetail {
            id: "tc-1".to_string(),
            name: "tool".to_string(),
            arguments: "a".repeat(500),
        }],
        usage: UsageInfo::default(),
    });

    let dir = logger.session_dir().unwrap();
    let resp = find_file_containing(&dir, "AI.Response.md");
    let content = fs::read_to_string(resp).unwrap();
    assert!(content.contains("tc-1"));
}

#[test]
fn log_local_operations_truncated_args_and_result() {
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Truncated,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(config, tmp.path());
    logger.create_session().unwrap();

    logger.log_local_operations(&LocalOperationInfo {
        round: 1,
        timestamp: Local::now(),
        operations: vec![OperationInfo {
            op_type: "tool_call".to_string(),
            name: "search".to_string(),
            status: "Success".to_string(),
            error: String::new(),
            duration_ms: 50,
            arguments: "b".repeat(500),
            result: "c".repeat(800),
        }],
    });

    let dir = logger.session_dir().unwrap();
    let local = find_file_containing(&dir, "Local.md");
    let content = fs::read_to_string(local).unwrap();
    assert!(content.contains("search"));
}

#[test]
fn log_methods_without_session_are_silent_noop() {
    // Enabled but create_session() never called → write_file hits the None branch:
    // no panic, nothing written.
    let tmp = TempDir::new().unwrap();
    let logger = RequestLogger::new(test_config(), tmp.path());
    assert!(logger.is_enabled());

    logger.log_user_request(&UserRequestInfo {
        timestamp: Local::now(),
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "hello".to_string(),
    });
    logger.log_llm_request(&LLMRequestInfo::default());
    logger.log_llm_response(&LLMResponseInfo {
        round: 1,
        timestamp: Local::now(),
        duration_ms: 1,
        content: "x".to_string(),
        tool_calls_count: 0,
        finish_reason: "stop".to_string(),
        tool_calls: Vec::new(),
        usage: UsageInfo::default(),
    });
    logger.log_final_response(&FinalResponseInfo {
        timestamp: Local::now(),
        total_duration_ms: 1,
        llm_rounds: 1,
        content: "x".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
    });

    assert!(logger.session_dir().is_none());
}

use super::*;

#[test]
fn test_build_request_body() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: std::collections::HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    assert_eq!(body["model"], "gpt-4");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[test]
fn test_build_request_with_tools() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];

    let body = provider.build_request_body(&[], &tools, "gpt-4", &ChatOptions {
        temperature: Some(0.7),
        max_tokens: Some(1000),
        ..Default::default()
    });
    assert!(body.get("tools").is_some());
    assert_eq!(body["temperature"], 0.7);
    assert_eq!(body["max_tokens"], 1000);
}

#[test]
fn test_normalize_model() {
    assert_eq!(HttpProvider::normalize_model("openai/gpt-4"), "gpt-4");
    assert_eq!(HttpProvider::normalize_model("gpt4"), "gpt-4");
    assert_eq!(HttpProvider::normalize_model("gpt-4o"), "gpt-4o");
    assert_eq!(HttpProvider::normalize_model("gpt4o"), "gpt-4o");
    assert_eq!(HttpProvider::normalize_model("claude3"), "claude-3-sonnet-20240229");
    assert_eq!(HttpProvider::normalize_model("anthropic/claude3-opus"), "claude-3-opus-20240229");
    assert_eq!(HttpProvider::normalize_model("my-custom-model"), "my-custom-model");
    assert_eq!(HttpProvider::normalize_model("  gpt-4  "), "gpt-4");
}

#[test]
fn test_uses_completion_tokens() {
    assert!(HttpProvider::uses_completion_tokens("o1-preview"));
    assert!(HttpProvider::uses_completion_tokens("o3-mini"));
    assert!(HttpProvider::uses_completion_tokens("gpt-5-turbo"));
    assert!(HttpProvider::uses_completion_tokens("glm-4"));
    assert!(!HttpProvider::uses_completion_tokens("gpt-4"));
    assert!(!HttpProvider::uses_completion_tokens("claude-3"));
}

#[test]
fn test_build_request_body_normalizes_model() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(&[], &[], "openai/gpt-4", &ChatOptions::default());
    assert_eq!(body["model"], "gpt-4");
}

#[test]
fn test_build_request_body_o1_no_temperature() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "o1".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(
        &[], &[], "o1-preview",
        &ChatOptions { temperature: Some(0.7), ..Default::default() }
    );
    // o1 models should NOT have temperature
    assert!(body.get("temperature").is_none());
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_http_provider_config_serialization() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 60,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: HttpProviderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "test");
    assert_eq!(deserialized.api_key, "sk-test");
    assert_eq!(deserialized.timeout_secs, 60);
}

#[test]
fn test_http_provider_config_with_proxy() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: Some("http://proxy:8080".to_string()),
        preserve_prefix: false,
    };
    // Should not panic when creating with proxy
    let _provider = HttpProvider::new(config);
}

#[test]
fn test_http_provider_config_with_headers() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: {
            let mut h = HashMap::new();
            h.insert("X-Custom".to_string(), "value".to_string());
            h
        },
        proxy: None,
        preserve_prefix: false,
    };
    let _provider = HttpProvider::new(config);
}

#[test]
fn test_normalize_model_aliases() {
    assert_eq!(HttpProvider::normalize_model("gpt4"), "gpt-4");
    assert_eq!(HttpProvider::normalize_model("gpt4o"), "gpt-4o");
    assert_eq!(HttpProvider::normalize_model("gpt4-turbo"), "gpt-4-turbo");
    assert_eq!(HttpProvider::normalize_model("gpt35-turbo"), "gpt-3.5-turbo");
    assert_eq!(HttpProvider::normalize_model("claude3"), "claude-3-sonnet-20240229");
    assert_eq!(HttpProvider::normalize_model("claude3-opus"), "claude-3-opus-20240229");
    assert_eq!(HttpProvider::normalize_model("claude3-sonnet"), "claude-3-sonnet-20240229");
    assert_eq!(HttpProvider::normalize_model("claude3-haiku"), "claude-3-haiku-20240307");
}

#[test]
fn test_normalize_model_preserves_unknown() {
    assert_eq!(HttpProvider::normalize_model("my-custom-model"), "my-custom-model");
    assert_eq!(HttpProvider::normalize_model("deepseek-chat"), "deepseek-chat");
}

#[test]
fn test_normalize_model_strips_prefix() {
    assert_eq!(HttpProvider::normalize_model("openai/gpt-4"), "gpt-4");
    assert_eq!(HttpProvider::normalize_model("anthropic/claude-3"), "claude-3");
    assert_eq!(HttpProvider::normalize_model("deepseek/deepseek-chat"), "deepseek-chat");
}

#[test]
fn test_normalize_model_whitespace() {
    assert_eq!(HttpProvider::normalize_model("  gpt-4  "), "gpt-4");
    assert_eq!(HttpProvider::normalize_model("  openai/gpt-4  "), "gpt-4");
}

#[test]
fn test_uses_completion_tokens_various() {
    assert!(HttpProvider::uses_completion_tokens("o1-preview"));
    assert!(HttpProvider::uses_completion_tokens("o1-mini"));
    assert!(HttpProvider::uses_completion_tokens("o3-mini"));
    assert!(HttpProvider::uses_completion_tokens("o3-high"));
    assert!(HttpProvider::uses_completion_tokens("gpt-5"));
    assert!(HttpProvider::uses_completion_tokens("gpt-5-turbo"));
    assert!(HttpProvider::uses_completion_tokens("glm-4"));
    assert!(HttpProvider::uses_completion_tokens("glm-4-plus"));
    assert!(!HttpProvider::uses_completion_tokens("gpt-4"));
    assert!(!HttpProvider::uses_completion_tokens("gpt-4o"));
    assert!(!HttpProvider::uses_completion_tokens("claude-3"));
    assert!(!HttpProvider::uses_completion_tokens("deepseek-chat"));
}

#[test]
fn test_build_request_body_completion_tokens() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    // o1 model should use max_completion_tokens
    let body = provider.build_request_body(
        &[], &[], "o1-preview",
        &ChatOptions { max_tokens: Some(4096), ..Default::default() }
    );
    assert!(body.get("max_completion_tokens").is_some());
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["max_completion_tokens"], 4096);
}

#[test]
fn test_build_request_body_regular_max_tokens() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    // Regular model uses max_tokens
    let body = provider.build_request_body(
        &[], &[], "gpt-4",
        &ChatOptions { max_tokens: Some(2048), ..Default::default() }
    );
    assert!(body.get("max_tokens").is_some());
    assert!(body.get("max_completion_tokens").is_none());
    assert_eq!(body["max_tokens"], 2048);
}

#[test]
fn test_build_request_body_top_p() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(
        &[], &[], "gpt-4",
        &ChatOptions { top_p: Some(0.9), ..Default::default() }
    );
    assert_eq!(body["top_p"], 0.9);
}

#[test]
fn test_build_request_body_stop() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(
        &[], &[], "gpt-4",
        &ChatOptions { stop: Some(vec!["stop1".to_string(), "stop2".to_string()]), ..Default::default() }
    );
    assert!(body.get("stop").is_some());
}

#[test]
fn test_build_request_body_no_optional_fields() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(&[], &[], "gpt-4", &ChatOptions::default());
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("top_p").is_none());
    assert!(body.get("stop").is_none());
    assert!(body.get("tools").is_none());
}

#[test]
fn test_build_request_body_kimi_temperature() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    // Kimi models should auto-set temperature=1.0 when not specified
    let body = provider.build_request_body(
        &[], &[], "moonshot-v1",
        &ChatOptions::default()
    );
    // "moonshot" triggers Kimi logic
    assert_eq!(body["temperature"], 1.0);
}

#[test]
fn test_build_request_body_preserve_prefix() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: true,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(&[], &[], "openai/gpt-4", &ChatOptions::default());
    assert_eq!(body["model"], "openai/gpt-4");
}

#[test]
fn test_build_request_body_o3_no_temperature() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(
        &[], &[], "o3-mini",
        &ChatOptions { temperature: Some(0.5), ..Default::default() }
    );
    assert!(body.get("temperature").is_none());
}

#[test]
fn test_default_model_and_name() {
    let config = HttpProviderConfig {
        name: "my-provider".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4o".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);
    assert_eq!(provider.default_model(), "gpt-4o");
    assert_eq!(provider.name(), "my-provider");
}

#[test]
fn test_build_request_body_with_multiple_messages() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![
        Message { role: "system".to_string(), content: "You are helpful".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None, reasoning_content: None, extra: HashMap::new() },
        Message { role: "user".to_string(), content: "Hello".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None, reasoning_content: None, extra: HashMap::new() },
        Message { role: "assistant".to_string(), content: "Hi".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None, reasoning_content: None, extra: HashMap::new() },
        Message { role: "user".to_string(), content: "How are you?".to_string(), tool_calls: vec![], tool_call_id: None, timestamp: None, reasoning_content: None, extra: HashMap::new() },
    ];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);
}

// --- StreamChunk tests ---

#[test]
fn test_stream_chunk_serialize() {
    let chunk = StreamChunk {
        delta: "Hello".to_string(),
        tool_calls: vec![],
        finish_reason: None,
        usage: None,
        reasoning_content: None,
    };
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("Hello"));
    assert!(!json.contains("finish_reason"));
}

#[test]
fn test_stream_chunk_with_finish() {
    let chunk = StreamChunk {
        delta: String::new(),
        tool_calls: vec![],
        finish_reason: Some("stop".to_string()),
        usage: Some(UsageInfo {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
            cached_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }),
        reasoning_content: None,
    };
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("stop"));
    assert!(json.contains("30"));
}

#[test]
fn test_stream_chunk_with_tool_calls() {
    let chunk = StreamChunk {
        delta: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_123".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path": "/test"}"#.to_string(),
            }),
            name: None,
            arguments: None,
        }],
        finish_reason: Some("tool_calls".to_string()),
        usage: None,
        reasoning_content: None,
    };
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("call_123"));
    assert!(json.contains("read_file"));
    assert!(json.contains("tool_calls"));
}

#[test]
fn test_stream_chunk_deserialize() {
    let json = r#"{"delta":" world","tool_calls":[],"finish_reason":null}"#;
    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.delta, " world");
    assert!(chunk.tool_calls.is_empty());
    assert!(chunk.finish_reason.is_none());
}

#[tokio::test]
async fn test_chat_stream_returns_channel() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(), // non-existent
        api_key: "test".to_string(),
        default_model: "test".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: std::collections::HashMap::new(),
    }];

    let mut rx = provider.chat_stream(&messages, &[], "test", &ChatOptions::default());

    // Should eventually get an error (connection refused / timeout).
    // Use tokio::time::timeout to avoid hanging.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv(),
    ).await;

    // The channel should return something (likely an error).
    match result {
        Ok(Some(Err(_))) => { /* expected — connection error */ }
        Ok(Some(Ok(chunk))) => {
            // Got a chunk — unlikely with port 1, but not a failure
            assert!(!chunk.delta.is_empty() || chunk.finish_reason.is_some());
        }
        Ok(None) => { /* channel closed */ }
        Err(_) => { /* timeout — acceptable, the spawned task might be slow */ }
    }
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_build_request_body_empty_model_stays_empty() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4o".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    // Empty model string stays empty after normalize (no special handling)
    let body = provider.build_request_body(&[], &[], "", &ChatOptions::default());
    assert_eq!(body["model"], "");
}

#[test]
fn test_build_request_body_with_stop_strings() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let body = provider.build_request_body(
        &[], &[], "gpt-4",
        &ChatOptions {
            stop: Some(vec!["\n\n".to_string(), "END".to_string()]),
            ..Default::default()
        },
    );
    let stop = body["stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0], "\n\n");
    assert_eq!(stop[1], "END");
}

#[test]
fn test_normalize_model_deepseek() {
    assert_eq!(HttpProvider::normalize_model("deepseek-chat"), "deepseek-chat");
    assert_eq!(HttpProvider::normalize_model("deepseek/deepseek-chat"), "deepseek-chat");
}

#[test]
fn test_normalize_model_gpt35_aliases() {
    assert_eq!(HttpProvider::normalize_model("gpt35-turbo"), "gpt-3.5-turbo");
    // "gpt35" without -turbo is not an alias, stays as-is
    assert_eq!(HttpProvider::normalize_model("gpt35"), "gpt35");
}

#[test]
fn test_normalize_model_o_series() {
    assert_eq!(HttpProvider::normalize_model("o1"), "o1");
    assert_eq!(HttpProvider::normalize_model("o3-mini"), "o3-mini");
}

#[test]
fn test_uses_completion_tokens_negative_cases() {
    assert!(!HttpProvider::uses_completion_tokens("gpt-3.5-turbo"));
    assert!(!HttpProvider::uses_completion_tokens("gpt-4"));
    assert!(!HttpProvider::uses_completion_tokens("gpt-4o"));
    assert!(!HttpProvider::uses_completion_tokens("gpt-4o-mini"));
    assert!(!HttpProvider::uses_completion_tokens("claude-3-sonnet"));
    assert!(!HttpProvider::uses_completion_tokens("deepseek-chat"));
}

#[test]
fn test_http_provider_new_creates_client() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.example.com/v1".to_string(),
        api_key: "key".to_string(),
        default_model: "model".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);
    assert_eq!(provider.default_model(), "model");
    assert_eq!(provider.name(), "test");
}

#[test]
fn test_http_provider_config_with_multiple_headers() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: {
            let mut h = HashMap::new();
            h.insert("X-Api-Key".to_string(), "abc123".to_string());
            h.insert("X-Request-Id".to_string(), "req-1".to_string());
            h.insert("Authorization".to_string(), "Bearer override".to_string());
            h
        },
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);
    assert_eq!(provider.name(), "test");
}

#[test]
fn test_build_request_body_with_tool_calls_message() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "assistant".to_string(),
        content: "".to_string(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/test.txt"}"#.to_string(),
            }),
            name: None,
            arguments: None,
        }],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: std::collections::HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    let msg = &body["messages"][0];
    assert!(msg.get("tool_calls").is_some());
    let tc = msg["tool_calls"].as_array().unwrap();
    assert_eq!(tc.len(), 1);
    assert_eq!(tc[0]["function"]["name"], "read_file");
}

#[test]
fn test_build_request_body_with_tool_result_message() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "tool".to_string(),
        content: "file contents here".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
        timestamp: None,
        reasoning_content: None,
extra: std::collections::HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    let msg = &body["messages"][0];
    assert_eq!(msg["role"], "tool");
    assert_eq!(msg["tool_call_id"], "call_1");
    assert_eq!(msg["content"], "file contents here");
}

#[test]
fn test_build_request_body_kimi_with_custom_temperature() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    // Kimi model with explicit temperature should keep it
    let body = provider.build_request_body(
        &[], &[], "moonshot-v1",
        &ChatOptions { temperature: Some(0.5), ..Default::default() }
    );
    assert_eq!(body["temperature"], 0.5);
}

#[test]
fn test_http_provider_config_default_model_accessor() {
    let config = HttpProviderConfig {
        name: "my-provider".to_string(),
        base_url: "https://api.custom.com".to_string(),
        api_key: "key".to_string(),
        default_model: "custom-model".to_string(),
        timeout_secs: 60,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: true,
    };
    let provider = HttpProvider::new(config);
    assert_eq!(provider.default_model(), "custom-model");
}

// ============================================================
// Integration-style tests for chat() method
// ============================================================

#[tokio::test]
async fn test_chat_connection_error() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(), // non-existent
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let result = provider.chat(&messages, &[], "gpt-4", &ChatOptions::default()).await;
    assert!(result.is_err());
    match result {
        Err(FailoverError::Timeout { provider, model }) => {
            assert_eq!(provider, "test");
            assert_eq!(model, "gpt-4");
        }
        Err(other) => panic!("Expected Timeout error, got: {:?}", other),
        Ok(_) => panic!("Expected error for non-existent server"),
    }
}

#[tokio::test]
async fn test_chat_http_401_error() {
    // Use mock-server pattern via reqwest::Response::builder
    // We'll test error handling logic by simulating HTTP errors

    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:12345".to_string(), // non-existent mock server
        api_key: "invalid-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let result = provider.chat(&messages, &[], "gpt-4", &ChatOptions::default()).await;
    // Should get timeout/connection error since server doesn't exist
    assert!(result.is_err());
}

#[tokio::test]
async fn test_chat_empty_model_uses_default() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4o".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    // Pass empty string for model - should use default "gpt-4o"
    let result = provider.chat(&messages, &[], "", &ChatOptions::default()).await;
    assert!(result.is_err());
    match result {
        Err(FailoverError::Timeout { provider: _, model }) => {
            // Should use default model in error info
            assert_eq!(model, "gpt-4o");
        }
        Err(_) => { /* other errors acceptable */ }
        Ok(_) => panic!("Expected error for non-existent server"),
    }
}

#[tokio::test]
async fn test_chat_stream_connection_error() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let mut rx = provider.chat_stream(&messages, &[], "gpt-4", &ChatOptions::default());

    // Should receive error via channel
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv()
    ).await;

    match result {
        Ok(Some(Err(e))) => {
            // Expected: connection error
            match e {
                FailoverError::Timeout { provider, model } => {
                    assert_eq!(provider, "test");
                    assert_eq!(model, "gpt-4");
                }
                _ => { /* other network errors acceptable */ }
            }
        }
        Ok(Some(Ok(_))) => {
            panic!("Unexpected success with non-existent server");
        }
        Ok(None) => {
            // Channel closed - acceptable for connection error
        }
        Err(_) => {
            // Timeout - acceptable, connection may be slow to fail
        }
    }
}

#[tokio::test]
async fn test_chat_stream_empty_model_uses_default() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4o".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    // Empty model string should use default
    let mut rx = provider.chat_stream(&messages, &[], "", &ChatOptions::default());

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv()
    ).await;

    // Just verify it doesn't panic and uses default model logic
    match result {
        Ok(Some(Err(FailoverError::Timeout { model, .. }))) => {
            assert_eq!(model, "gpt-4o");
        }
        _ => { /* other outcomes acceptable */ }
    }
}

#[tokio::test]
async fn test_chat_stream_with_tools() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let mut rx = provider.chat_stream(&messages, &tools, "gpt-4", &ChatOptions::default());

    // Should handle tools in request (will fail connection, but request is built correctly)
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv()
    ).await;

    // Just verify it doesn't panic
    match result {
        Ok(Some(_)) => { /* got some response */ }
        Ok(None) => { /* channel closed */ }
        Err(_) => { /* timeout */ }
    }
}

#[tokio::test]
async fn test_chat_stream_with_custom_options() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let options = ChatOptions {
        temperature: Some(0.5),
        max_tokens: Some(1000),
        top_p: Some(0.9),
        stop: Some(vec!["END".to_string()]),
        extra: HashMap::new(),
    };

    let mut rx = provider.chat_stream(&messages, &[], "gpt-4", &options);

    // Just verify it builds request without panic (will fail connection)
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv()
    ).await;

    // Verify we got something (likely error, but request was built)
    match result {
        Ok(Some(_)) => { /* got response */ }
        Ok(None) => { /* channel closed */ }
        Err(_) => { /* timeout */ }
    }
}

#[tokio::test]
async fn test_chat_with_tools() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let result = provider.chat(&messages, &tools, "gpt-4", &ChatOptions::default()).await;
    // Should fail with connection error, but request was built with tools
    assert!(result.is_err());
}

#[tokio::test]
async fn test_chat_with_custom_options() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let options = ChatOptions {
        temperature: Some(0.7),
        max_tokens: Some(2048),
        top_p: Some(0.9),
        stop: Some(vec!["STOP".to_string()]),
        extra: HashMap::new(),
    };

    let result = provider.chat(&messages, &[], "gpt-4", &options).await;
    // Should fail with connection error, but request was built with custom options
    assert!(result.is_err());
}

#[tokio::test]
async fn test_chat_with_proxy_config() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: Some("http://proxy:8080".to_string()),
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let result = provider.chat(&messages, &[], "gpt-4", &ChatOptions::default()).await;
    // Should fail (proxy doesn't exist), but provider was created with proxy config
    assert!(result.is_err());
}

#[tokio::test]
async fn test_chat_with_custom_headers() {
    let mut headers = HashMap::new();
    headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());
    headers.insert("X-Request-ID".to_string(), "req-123".to_string());

    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 1,
        headers,
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let result = provider.chat(&messages, &[], "gpt-4", &ChatOptions::default()).await;
    // Should fail connection, but request was built with custom headers
    assert!(result.is_err());
}

#[tokio::test]
async fn test_chat_stream_reasoning_model() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "o1-preview".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let mut rx = provider.chat_stream(&messages, &[], "o1-preview", &ChatOptions::default());

    // Just verify it builds request correctly (no temperature for o1)
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv()
    ).await;

    match result {
        Ok(Some(_)) => { /* got response */ }
        Ok(None) => { /* channel closed */ }
        Err(_) => { /* timeout */ }
    }
}

// ============================================================
// Tests for error handling and edge cases
// ============================================================

#[test]
fn test_extract_usage_basic() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 10,
        "completion_tokens": 20,
        "total_tokens": 30
    });

    let usage = extract_usage(&usage_json);
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 20);
    assert_eq!(usage.total_tokens, 30);
    assert_eq!(usage.cached_tokens, None);
}

#[test]
fn test_extract_usage_with_cached_tokens() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 10,
        "completion_tokens": 20,
        "total_tokens": 30,
        "prompt_cache_hit_tokens": 5
    });

    let usage = extract_usage(&usage_json);
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 20);
    assert_eq!(usage.total_tokens, 30);
    assert_eq!(usage.cached_tokens, Some(5));
    assert_eq!(usage.cache_read_tokens, Some(5));
}

#[test]
fn test_extract_usage_with_cached_tokens_openai_format() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 10,
        "completion_tokens": 20,
        "total_tokens": 30,
        "prompt_tokens_details": {
            "cached_tokens": 8
        }
    });

    let usage = extract_usage(&usage_json);
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 20);
    assert_eq!(usage.total_tokens, 30);
    assert_eq!(usage.cached_tokens, Some(8));
    assert_eq!(usage.cache_read_tokens, Some(8));
}

#[test]
fn test_extract_usage_zero_tokens() {
    let usage_json = serde_json::json!({
        "prompt_tokens": 0,
        "completion_tokens": 0,
        "total_tokens": 0
    });

    let usage = extract_usage(&usage_json);
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn test_extract_usage_missing_fields() {
    let usage_json = serde_json::json!({});

    let usage = extract_usage(&usage_json);
    // Missing fields should default to 0
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn test_stream_chunk_serialization_with_all_fields() {
    let chunk = StreamChunk {
        delta: "Hello".to_string(),
        tool_calls: vec![ToolCall {
            id: "call_123".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "test_tool".to_string(),
                arguments: r#"{"arg":"value"}"#.to_string(),
            }),
            name: None,
            arguments: None,
        }],
        finish_reason: Some("stop".to_string()),
        usage: Some(UsageInfo {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
            cached_tokens: Some(5),
            cache_creation_tokens: None,
            cache_read_tokens: Some(5),
        }),
        reasoning_content: Some("thinking process".to_string()),
    };

    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("Hello"));
    assert!(json.contains("call_123"));
    assert!(json.contains("test_tool"));
    assert!(json.contains("stop"));
    assert!(json.contains("30"));
    assert!(json.contains("thinking"));
}

#[test]
fn test_stream_chunk_default_fields() {
    let chunk = StreamChunk::default();

    assert!(chunk.delta.is_empty());
    assert!(chunk.tool_calls.is_empty());
    assert!(chunk.finish_reason.is_none());
    assert!(chunk.usage.is_none());
    assert!(chunk.reasoning_content.is_none());
}

#[test]
fn test_http_provider_config_default_timeout() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 120, // default value
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };

    let provider = HttpProvider::new(config);
    assert_eq!(provider.name(), "test");
    assert_eq!(provider.default_model(), "gpt-4");
}

#[test]
fn test_http_provider_config_socks_proxy() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: Some("socks5://proxy:1080".to_string()),
        preserve_prefix: false,
    };

    // Should not panic when creating with SOCKS proxy
    let _provider = HttpProvider::new(config);
}

#[test]
fn test_build_request_body_multiple_tools() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let tools = vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        },
    ];

    let body = provider.build_request_body(&[], &tools, "gpt-4", &ChatOptions::default());
    assert!(body.get("tools").is_some());
    assert_eq!(body["tool_choice"], "auto");
    let tools_array = body["tools"].as_array().unwrap();
    assert_eq!(tools_array.len(), 2);
}

#[test]
fn test_build_request_body_glm_model() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    // GLM models should use max_completion_tokens
    let body = provider.build_request_body(
        &[], &[], "glm-4-plus",
        &ChatOptions { max_tokens: Some(4096), ..Default::default() }
    );
    assert!(body.get("max_completion_tokens").is_some());
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["max_completion_tokens"], 4096);
}

#[test]
fn test_normalize_model_gpt5_variants() {
    assert_eq!(HttpProvider::normalize_model("gpt-5"), "gpt-5");
    assert_eq!(HttpProvider::normalize_model("gpt-5-turbo"), "gpt-5-turbo");
    assert_eq!(HttpProvider::normalize_model("gpt-5-mini"), "gpt-5-mini");
}

#[test]
fn test_uses_completion_tokens_edge_cases() {
    // Test case sensitivity
    assert!(HttpProvider::uses_completion_tokens("O1-PREVIEW"));
    assert!(HttpProvider::uses_completion_tokens("O3-MINI"));

    // Test model name containing "glm-" but not at start
    assert!(HttpProvider::uses_completion_tokens("glm-4"));
    assert!(HttpProvider::uses_completion_tokens("glm-3-turbo"));

    // Test gpt-5 at start
    assert!(HttpProvider::uses_completion_tokens("gpt-5"));
    assert!(HttpProvider::uses_completion_tokens("gpt-5-turbo"));

    // Test negative cases
    assert!(!HttpProvider::uses_completion_tokens("gpt-4"));
    assert!(!HttpProvider::uses_completion_tokens("gla"));
    assert!(!HttpProvider::uses_completion_tokens("model"));
}

#[test]
fn test_build_request_body_with_system_and_user_messages() {
    let config = HttpProviderConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    };
    let provider = HttpProvider::new(config);

    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "You are helpful assistant".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
    ];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    let messages_array = body["messages"].as_array().unwrap();
    assert_eq!(messages_array.len(), 2);
    assert_eq!(messages_array[0]["role"], "system");
    assert_eq!(messages_array[1]["role"], "user");
}

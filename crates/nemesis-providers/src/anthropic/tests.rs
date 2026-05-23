use super::*;

#[test]
fn test_normalize_base_url() {
    assert_eq!(normalize_base_url(""), DEFAULT_BASE_URL);
    assert_eq!(normalize_base_url("https://api.anthropic.com/v1"), "https://api.anthropic.com");
    assert_eq!(normalize_base_url("https://custom.api.com/"), "https://custom.api.com");
    assert_eq!(normalize_base_url("  https://api.anthropic.com/v1/  "), "https://api.anthropic.com");
}

#[test]
fn test_build_request_body_simple() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    assert_eq!(body["model"], "claude-3");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["max_tokens"], 4096);
}

#[test]
fn test_build_request_body_with_system() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hi".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
extra: HashMap::new(),
        },
    ];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    assert!(body.get("system").is_some());
    let system = body["system"].as_array().unwrap();
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["type"], "text");
}

#[test]
fn test_translate_tools() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
    }];
    let translated = translate_tools(&tools);
    assert_eq!(translated.len(), 1);
    assert_eq!(translated[0]["name"], "read_file");
    assert_eq!(translated[0]["description"], "Read a file");
    assert!(translated[0].get("input_schema").is_some());
}

#[test]
fn test_parse_response_text_only() {
    let data = serde_json::json!({
        "content": [{"type": "text", "text": "Hello!"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 5}
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "Hello!");
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.usage.unwrap().total_tokens, 15);
}

#[test]
fn test_parse_response_tool_use() {
    let data = serde_json::json!({
        "content": [
            {"type": "text", "text": "Using tool"},
            {"type": "tool_use", "id": "tu_123", "name": "read_file", "input": {"path": "/tmp"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 20, "output_tokens": 10}
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "Using tool");
    assert_eq!(resp.finish_reason, "tool_calls");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "tu_123");
    assert_eq!(resp.tool_calls[0].name.as_deref(), Some("read_file"));
}

#[test]
fn test_parse_response_max_tokens() {
    let data = serde_json::json!({
        "content": [{"type": "text", "text": "Truncated"}],
        "stop_reason": "max_tokens",
        "usage": {"input_tokens": 10, "output_tokens": 100}
    });
    let resp = parse_response(&data);
    assert_eq!(resp.finish_reason, "length");
}

#[test]
fn test_anthropic_config_default() {
    let config = AnthropicConfig::default();
    assert_eq!(config.base_url, DEFAULT_BASE_URL);
    assert_eq!(config.default_model, DEFAULT_MODEL);
    assert_eq!(config.timeout_secs, 120);
}

#[test]
fn test_with_token_source_and_base_url() {
    let config = AnthropicConfig::default();
    let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
        Box::new(|| Ok("refreshed-token".to_string()));
    let provider = AnthropicProvider::with_token_source_and_base_url(
        config,
        ts,
        "https://custom.api.com/v1/",
    );
    assert_eq!(provider.base_url(), "https://custom.api.com");
    assert!(provider.token_source.is_some());
}

#[test]
fn test_with_token_source_and_base_url_empty() {
    let config = AnthropicConfig::default();
    let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
        Box::new(|| Ok("token".to_string()));
    let provider = AnthropicProvider::with_token_source_and_base_url(config, ts, "");
    // Empty base_url should keep the config default
    assert_eq!(provider.base_url(), DEFAULT_BASE_URL);
}

#[test]
fn test_base_url_method() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    assert_eq!(provider.base_url(), DEFAULT_BASE_URL);
}

// -- Additional tests --

#[test]
fn test_anthropic_config_serialization_roundtrip() {
    let config = AnthropicConfig {
        api_key: "sk-ant-test".into(),
        base_url: "https://custom.api.com".into(),
        default_model: "claude-3-opus".into(),
        timeout_secs: 60,
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: AnthropicConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.api_key, "sk-ant-test");
    assert_eq!(back.base_url, "https://custom.api.com");
    assert_eq!(back.default_model, "claude-3-opus");
    assert_eq!(back.timeout_secs, 60);
}

#[test]
fn test_anthropic_config_deserialization_partial() {
    let json = r#"{"api_key": "sk-ant-test"}"#;
    let config: AnthropicConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.api_key, "sk-ant-test");
    assert_eq!(config.base_url, ""); // serde default = empty string
    assert_eq!(config.default_model, ""); // serde default = empty string
    assert_eq!(config.timeout_secs, 120);
}

#[test]
fn test_normalize_base_url_trailing_slash() {
    assert_eq!(normalize_base_url("https://api.anthropic.com/"), "https://api.anthropic.com");
    // /v1 gets stripped too
    assert_eq!(normalize_base_url("https://api.anthropic.com/v1/"), "https://api.anthropic.com");
    assert_eq!(normalize_base_url("https://api.anthropic.com/v1"), "https://api.anthropic.com");
}

#[test]
fn test_normalize_base_url_no_trailing_slash() {
    assert_eq!(normalize_base_url("https://api.anthropic.com"), "https://api.anthropic.com");
}

#[test]
fn test_normalize_base_url_empty() {
    assert_eq!(normalize_base_url(""), DEFAULT_BASE_URL);
}

#[test]
fn test_parse_response_no_usage() {
    let data = serde_json::json!({
        "content": [{"type": "text", "text": "Hello!"}],
        "stop_reason": "end_turn"
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "Hello!");
    assert!(resp.usage.is_none());
}

#[test]
fn test_parse_response_empty_content() {
    let data = serde_json::json!({
        "content": [],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 5}
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_parse_response_text_and_tool_use() {
    let data = serde_json::json!({
        "content": [
            {"type": "text", "text": "Let me check"},
            {"type": "tool_use", "id": "tu_1", "name": "search", "input": {"q": "test"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 15, "output_tokens": 8}
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "Let me check");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.finish_reason, "tool_calls");
    assert_eq!(resp.usage.unwrap().total_tokens, 23);
}

#[test]
fn test_translate_tools_empty() {
    let tools: Vec<ToolDefinition> = vec![];
    let translated = translate_tools(&tools);
    assert!(translated.is_empty());
}

#[test]
fn test_default_constants() {
    assert_eq!(DEFAULT_BASE_URL, "https://api.anthropic.com");
    assert_eq!(DEFAULT_MODEL, "claude-sonnet-4-5-20250929");
}

#[test]
fn test_provider_name() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    assert_eq!(provider.name(), "anthropic");
}

#[test]
fn test_provider_default_model() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    assert_eq!(provider.default_model(), DEFAULT_MODEL);
}

// ---- Additional coverage for edge cases ----

#[test]
fn test_build_request_body_user_with_tool_call_id() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "file result data".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("tu_123".to_string()),
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "user");
    let content = msgs[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "tu_123");
}

#[test]
fn test_build_request_body_tool_with_call_id() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "tool".to_string(),
        content: "tool output".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("tu_456".to_string()),
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "user");
    let content = msgs[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "tu_456");
}

#[test]
fn test_build_request_body_tool_without_call_id() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "tool".to_string(),
        content: "orphan output".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let msgs = body["messages"].as_array().unwrap();
    assert!(msgs.is_empty());
}

#[test]
fn test_build_request_body_unknown_role() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "custom_role".to_string(),
        content: "ignored".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let msgs = body["messages"].as_array().unwrap();
    assert!(msgs.is_empty());
}

#[test]
fn test_build_request_body_with_temperature() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let body = provider.build_request_body(&[], &[], "claude-3", &ChatOptions {
        temperature: Some(0.5),
        ..Default::default()
    });
    assert_eq!(body["temperature"], 0.5);
}

#[test]
fn test_build_request_body_with_tools() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]}),
        },
    }];
    let body = provider.build_request_body(&[], &tools, "claude-3", &ChatOptions::default());
    assert!(body.get("tools").is_some());
    let tools_arr = body["tools"].as_array().unwrap();
    assert_eq!(tools_arr.len(), 1);
    assert_eq!(tools_arr[0]["name"], "test_tool");
    assert!(tools_arr[0]["input_schema"].get("required").is_some());
}

#[test]
fn test_translate_tools_non_function_skipped() {
    let tools = vec![ToolDefinition {
        tool_type: "other".to_string(),
        function: ToolFunctionDefinition {
            name: "skipped".to_string(),
            description: "Should be skipped".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools(&tools);
    assert!(translated.is_empty());
}

#[test]
fn test_translate_tools_no_description() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "no_desc".to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools(&tools);
    assert_eq!(translated.len(), 1);
    assert!(translated[0].get("description").is_none());
}

#[test]
fn test_translate_tools_no_required_field() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "no_req".to_string(),
            description: "No required field".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        },
    }];
    let translated = translate_tools(&tools);
    assert_eq!(translated.len(), 1);
    assert!(translated[0]["input_schema"].get("required").is_none());
}

#[test]
fn test_parse_response_tool_use_with_invalid_input() {
    let data = serde_json::json!({
        "content": [
            {"type": "tool_use", "id": "tu_bad", "name": "test", "input": "not an object"}
        ],
        "stop_reason": "tool_use"
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    // Invalid input should get raw fallback
    assert!(resp.tool_calls[0].arguments.is_some());
    assert!(resp.tool_calls[0].arguments.as_ref().unwrap().contains_key("raw"));
}

#[test]
fn test_parse_response_unknown_block_type() {
    let data = serde_json::json!({
        "content": [
            {"type": "unknown_block", "data": "something"}
        ],
        "stop_reason": "end_turn"
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_parse_response_stop_reason_end_turn() {
    let data = serde_json::json!({
        "content": [{"type": "text", "text": "done"}],
        "stop_reason": "end_turn"
    });
    let resp = parse_response(&data);
    assert_eq!(resp.finish_reason, "stop");
}

#[test]
fn test_parse_response_stop_reason_unknown() {
    let data = serde_json::json!({
        "content": [{"type": "text", "text": "done"}],
        "stop_reason": "unknown_reason"
    });
    let resp = parse_response(&data);
    assert_eq!(resp.finish_reason, "stop");
}

#[test]
fn test_parse_response_no_stop_reason() {
    let data = serde_json::json!({
        "content": [{"type": "text", "text": "no stop"}]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.finish_reason, "stop");
}

#[test]
fn test_get_api_key_no_token_source() {
    let provider = AnthropicProvider::new(AnthropicConfig {
        api_key: "direct-key".to_string(),
        ..Default::default()
    });
    assert_eq!(provider.get_api_key().unwrap(), "direct-key");
}

#[test]
fn test_get_api_key_with_token_source() {
    let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
        Box::new(|| Ok("dynamic-key".to_string()));
    let provider = AnthropicProvider::with_token_source(
        AnthropicConfig::default(),
        ts,
    );
    assert_eq!(provider.get_api_key().unwrap(), "dynamic-key");
}

#[test]
fn test_get_api_key_with_failing_token_source() {
    let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
        Box::new(|| Err("token refresh failed".to_string()));
    let provider = AnthropicProvider::with_token_source(
        AnthropicConfig::default(),
        ts,
    );
    assert!(provider.get_api_key().is_err());
}

#[test]
fn test_normalize_base_url_only_v1() {
    assert_eq!(normalize_base_url("/v1"), DEFAULT_BASE_URL);
    assert_eq!(normalize_base_url("  /v1/  "), DEFAULT_BASE_URL);
}

#[test]
fn test_assistant_with_tool_calls_and_function_fallback() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    // ToolCall with no name field, but has function.name
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "tc_1".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "search".to_string(),
                arguments: r#"{"q":"test"}"#.to_string(),
            }),
            name: None, // name is None, should fallback to function.name
            arguments: None, // arguments is None, should produce empty json
        }],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "assistant");
    let content = msgs[0]["content"].as_array().unwrap();
    // Empty content should not produce text block, only tool_use block
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["name"], "search"); // from function.name fallback
}

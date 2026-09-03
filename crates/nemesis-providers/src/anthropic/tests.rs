use super::*;

#[test]
fn test_normalize_base_url() {
    assert_eq!(normalize_base_url(""), DEFAULT_BASE_URL);
    assert_eq!(
        normalize_base_url("https://api.anthropic.com/v1"),
        "https://api.anthropic.com"
    );
    assert_eq!(
        normalize_base_url("https://custom.api.com/"),
        "https://custom.api.com"
    );
    assert_eq!(
        normalize_base_url("  https://api.anthropic.com/v1/  "),
        "https://api.anthropic.com"
    );
}

#[test]
fn test_build_request_body_simple() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".into(),
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
            content: "You are helpful".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hi".into(),
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
    let provider =
        AnthropicProvider::with_token_source_and_base_url(config, ts, "https://custom.api.com/v1/");
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
    assert_eq!(
        normalize_base_url("https://api.anthropic.com/"),
        "https://api.anthropic.com"
    );
    // /v1 gets stripped too
    assert_eq!(
        normalize_base_url("https://api.anthropic.com/v1/"),
        "https://api.anthropic.com"
    );
    assert_eq!(
        normalize_base_url("https://api.anthropic.com/v1"),
        "https://api.anthropic.com"
    );
}

#[test]
fn test_normalize_base_url_no_trailing_slash() {
    assert_eq!(
        normalize_base_url("https://api.anthropic.com"),
        "https://api.anthropic.com"
    );
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
        content: "file result data".into(),
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
        content: "tool output".into(),
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
        content: "orphan output".into(),
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
        content: "ignored".into(),
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
    let body = provider.build_request_body(
        &[],
        &[],
        "claude-3",
        &ChatOptions {
            temperature: Some(0.5),
            ..Default::default()
        },
    );
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
    assert!(
        resp.tool_calls[0]
            .arguments
            .as_ref()
            .unwrap()
            .contains_key("raw")
    );
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
    let provider = AnthropicProvider::with_token_source(AnthropicConfig::default(), ts);
    assert_eq!(provider.get_api_key().unwrap(), "dynamic-key");
}

#[test]
fn test_get_api_key_with_failing_token_source() {
    let ts: Box<dyn Fn() -> Result<String, String> + Send + Sync> =
        Box::new(|| Err("token refresh failed".to_string()));
    let provider = AnthropicProvider::with_token_source(AnthropicConfig::default(), ts);
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
        content: String::new().into(),
        tool_calls: vec![ToolCall {
            id: "tc_1".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "search".to_string(),
                arguments: r#"{"q":"test"}"#.to_string(),
            }),
            name: None,      // name is None, should fallback to function.name
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

// H4 (U16 half): reasoning effort → thinking budget mapping
#[test]
fn test_effort_anthropic_budget_mapping() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];
    let mk = |tier: Option<&str>| {
        let mut o = ChatOptions::default();
        o.reasoning_effort = tier.map(|t| t.to_string());
        provider.build_request_body(&messages, &[], "claude-3", &o)
    };
    // Fixed tier→budget map.
    for (tier, budget) in [("low", 1024), ("medium", 4096), ("high", 16384)] {
        let b = mk(Some(tier));
        assert_eq!(b["thinking"]["type"], "enabled", "tier {tier}");
        assert_eq!(b["thinking"]["budget_tokens"], budget, "tier {tier}");
    }
    // None / "off" / unknown → no thinking block.
    for none_case in [mk(None), mk(Some("off")), mk(Some("banana"))] {
        assert!(
            none_case.get("thinking").is_none(),
            "no thinking block for unset/off/unknown"
        );
    }
}

use serde_json::json;

// ===========================================================================
// W4c 补测（2026-08-25）：chat() HTTP 矩阵（wiremock）+ build_request_body
// assistant/tool 消息分支
// ===========================================================================

use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn anth_config(base: &str) -> AnthropicConfig {
    AnthropicConfig {
        api_key: "ak-test".to_string(),
        base_url: base.to_string(),
        default_model: "claude-default".to_string(),
        timeout_secs: 10,
    }
}

fn anth_messages() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: "hi".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }]
}

#[tokio::test]
async fn test_w4c_anth_chat_success_headers_and_default_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "ak-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(body_partial_json(json!({"model": "claude-default"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                {"type": "text", "text": "hello-from-anthropic"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 2, "output_tokens": 3}
        })))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(anth_config(&server.uri()));
    // model 空 → default_model
    let resp = provider
        .chat(&anth_messages(), &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "hello-from-anthropic");
    // anthropic 的 end_turn 归一化为 openai 风格的 "stop"
    assert_eq!(resp.finish_reason, "stop");
}

#[tokio::test]
async fn test_w4c_anth_chat_429_maps_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(anth_config(&server.uri()));
    let err = provider
        .chat(&anth_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FailoverError::RateLimit { .. }));
}

#[tokio::test]
async fn test_w4c_anth_chat_invalid_json_maps_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<<<garbage"))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(anth_config(&server.uri()));
    let err = provider
        .chat(&anth_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FailoverError::Format { .. }));
}

#[tokio::test]
async fn test_w4c_anth_chat_dead_port_maps_timeout() {
    let provider = AnthropicProvider::new(anth_config("http://127.0.0.1:1"));
    let err = provider
        .chat(&anth_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FailoverError::Timeout { .. }));
}

#[test]
fn test_w4c_anth_build_body_assistant_tool_calls_and_tool_result() {
    let provider = AnthropicProvider::new(anth_config("http://unused"));

    let mut tc = ToolCall {
        id: "tc-1".to_string(),
        call_type: None,
        function: None,
        name: Some("get_weather".to_string()),
        arguments: Some(HashMap::from([(
            "city".to_string(),
            serde_json::json!("北京"),
        )])),
    };
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: "let me check".into(),
            tool_calls: vec![tc.clone()],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "assistant".to_string(),
            content: String::new().into(),
            tool_calls: vec![tc.clone()],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "tool".to_string(),
            content: "{\"temp\": 25}".into(),
            tool_calls: vec![],
            tool_call_id: Some("tc-1".to_string()),
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
    ];

    let body = provider.build_request_body(&messages, &[], "m", &ChatOptions::default());
    let api_msgs = body["messages"].as_array().unwrap();

    // 第一条 assistant：text 块 + tool_use 块
    let first = &api_msgs[0];
    assert_eq!(first["role"], "assistant");
    let blocks = first["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "let me check");
    assert_eq!(blocks[1]["type"], "tool_use");
    assert_eq!(blocks[1]["id"], "tc-1");
    assert_eq!(blocks[1]["name"], "get_weather");
    assert_eq!(blocks[1]["input"]["city"], "北京");

    // 第二条 assistant：content 为空 → 只有 tool_use 块（无空 text 块）
    let second = &api_msgs[1];
    let blocks2 = second["content"].as_array().unwrap();
    assert_eq!(blocks2.len(), 1);
    assert_eq!(blocks2[0]["type"], "tool_use");

    // tool 结果 → role=user + tool_result
    let third = &api_msgs[2];
    assert_eq!(third["role"], "user");
    let blocks3 = third["content"].as_array().unwrap();
    assert_eq!(blocks3[0]["type"], "tool_result");
    assert_eq!(blocks3[0]["tool_use_id"], "tc-1");
    assert_eq!(blocks3[0]["content"], "{\"temp\": 25}");

    // name 兜底链：function.name 路径
    tc.name = None;
    tc.arguments = None;
    tc.function = Some(FunctionCall {
        name: "fn-name".to_string(),
        arguments: "{}".to_string(),
    });
    let msgs2 = vec![Message {
        role: "assistant".to_string(),
        content: String::new().into(),
        tool_calls: vec![tc],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];
    let body2 = provider.build_request_body(&msgs2, &[], "m", &ChatOptions::default());
    let b = body2["messages"][0]["content"][0].clone();
    assert_eq!(b["name"], "fn-name");
    // arguments 为 None → 空 object
    assert_eq!(b["input"], serde_json::json!({}));
}

// ============================================================
// 字节快照（goal T1/T3 纪律 6）：content 类型多态化
// （String → MessageContent）前后，纯文本请求体必须逐字节一致
// ——这是 prompt cache 前缀契约（真相源 §1.5.2）。本测试先于
// 类型改动写好并锁死当前字节；类型改动后必须原样保持绿。
// 注：请求体经 serde_json::Value（无 preserve_order → BTreeMap），
// 键序为字母序——快照锁定的是"当前真实字节"而非声明序。
// ============================================================

#[test]
fn test_request_body_bytesnapshot_pure_text() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "You are helpful.".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hello".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "assistant".to_string(),
            content: "Hi there!".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
    ];

    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let serialized = serde_json::to_string(&body).unwrap();
    assert_eq!(
        serialized,
        r#"{"max_tokens":4096,"messages":[{"content":"Hello","role":"user"},{"content":"Hi there!","role":"assistant"}],"model":"claude-3","system":[{"text":"You are helpful.","type":"text"}]}"#
    );
}

// ============================================================
// T3：Anthropic image blocks（goal T3）
// ============================================================

#[test]
fn test_request_body_image_parts_anthropic_blocks() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "这是什么".to_string(),
            },
            ContentPart::Image {
                image: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                },
                detail: Some(ImageDetail::Low), // Anthropic 无 detail 字段 → 不透传
            },
            ContentPart::Image {
                image: ImageSource::Url("https://example.com/b.jpg".to_string()),
                detail: None,
            },
        ]),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let content = body["messages"][0]["content"]
        .as_array()
        .expect("blocks 数组");
    assert_eq!(content.len(), 3);

    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "这是什么");

    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert_eq!(content[1]["source"]["data"], "aGVsbG8=");
    assert!(content[1].get("detail").is_none());

    assert_eq!(content[2]["type"], "image");
    assert_eq!(content[2]["source"]["type"], "url");
    assert_eq!(content[2]["source"]["url"], "https://example.com/b.jpg");
}

#[test]
fn test_request_body_tool_result_content_stays_string() {
    // tool result content 不转 blocks（工具结果文本;MCP 图像属 P5 范围）
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text("工具输出".to_string()),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let msg = &body["messages"][0];
    assert_eq!(msg["content"][0]["type"], "tool_result");
    assert_eq!(msg["content"][0]["content"], "工具输出");
}

// 2026-09-03 二次回归 F1 回归锁：非 user 位（system / assistant / tool_result）
// 携带 Parts 时不得 raw 序列化产出非法 wire 块——文本视图（+诚实注记，D7 同族），
// 全请求无 base64 泄漏；Text 形态保持原字节（既有快照测试覆盖）。
#[test]
fn test_request_body_non_user_roles_with_parts_degrade_to_text_note() {
    let provider = AnthropicProvider::new(AnthropicConfig::default());
    let mk = |role: &str, content: MessageContent, tool_call_id: Option<String>| Message {
        role: role.to_string(),
        content,
        tool_calls: vec![],
        tool_call_id,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    };
    let parts = |text: &str, b64: &str| {
        MessageContent::Parts(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::Image {
                image: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: b64.to_string(),
                },
                detail: None,
            },
        ])
    };

    let messages = vec![
        mk("system", parts("系统指令", "aGVsbG8="), None),
        // user 位正常转 blocks（对照组；用独立 base64 标记区分「正常保留」与「泄漏」）
        mk("user", parts("看看这张图", "Y29udHJvbA=="), None),
        mk("assistant", parts("回答", "aGVsbG8="), None),
        mk(
            "user",
            parts("工具结果", "aGVsbG8="),
            Some("call_1".to_string()),
        ),
        mk(
            "tool",
            parts("工具结果2", "aGVsbG8="),
            Some("call_2".to_string()),
        ),
    ];

    let body = provider.build_request_body(&messages, &[], "claude-3", &ChatOptions::default());
    let raw = serde_json::to_string(&body).unwrap();

    // 非 user 位不得出现 image block / base64 泄漏（非 user 标记全请求消失）
    assert!(!raw.contains("aGVsbG8="), "base64 泄漏进请求: {raw}");
    // user 位对照组的图片照常保留（降级只作用于非 user 位）
    assert!(raw.contains("Y29udHJvbA=="), "user 位图片被误剥: {raw}");

    // system：**纯文本视图**（to_text）——system 是内部指令位，无"用户附带"
    // 语义，故无注记（与 codex system 分支同语义；回归 F1 的一致性修正）。
    assert_eq!(body["system"][0]["type"], "text");
    let sys_text = body["system"][0]["text"].as_str().unwrap();
    assert_eq!(sys_text, "系统指令", "system 位应取纯文本、无图片注记");

    // assistant 无 tool_calls：content 保持字符串 + 注记
    let asst = &body["messages"][1];
    assert_eq!(asst["role"], "assistant");
    assert!(asst["content"].is_string());
    assert!(asst["content"].as_str().unwrap().contains("1 张图片"));

    // user/tool 两个 tool_result 位：content 字符串 + 注记
    let tr1 = &body["messages"][2];
    assert_eq!(tr1["content"][0]["type"], "tool_result");
    assert!(
        tr1["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("1 张图片")
    );
    let tr2 = &body["messages"][3];
    assert_eq!(tr2["content"][0]["type"], "tool_result");
    assert!(
        tr2["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("1 张图片")
    );

    // 对照组：user 位 Parts 正常转 blocks（image block 仍在，证明降级只作用于非 user 位）
    let user = &body["messages"][0];
    let ucontent = user["content"].as_array().expect("user 位 blocks 数组");
    assert_eq!(ucontent[1]["type"], "image");
}

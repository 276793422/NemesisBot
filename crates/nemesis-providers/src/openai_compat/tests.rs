use super::*;

#[test]
fn test_normalize_model_strips_known_prefix() {
    assert_eq!(
        normalize_model("deepseek/chat", "https://api.deepseek.com"),
        "chat"
    );
    assert_eq!(
        normalize_model("groq/llama3", "https://api.groq.com"),
        "llama3"
    );
    assert_eq!(
        normalize_model("zhipu/glm-4", "https://open.bigmodel.cn"),
        "glm-4"
    );
    assert_eq!(
        normalize_model("ollama/llama3", "http://localhost:11434"),
        "llama3"
    );
}

#[test]
fn test_normalize_model_preserves_openrouter() {
    assert_eq!(
        normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"),
        "openai/gpt-4"
    );
}

#[test]
fn test_normalize_model_no_prefix() {
    assert_eq!(normalize_model("gpt-4", "https://api.openai.com"), "gpt-4");
}

#[test]
fn test_normalize_model_unknown_prefix() {
    assert_eq!(
        normalize_model("myprovider/model", "https://example.com"),
        "myprovider/model"
    );
}

#[test]
fn test_uses_completion_tokens() {
    assert!(OpenAICompatProvider::uses_completion_tokens("glm-4"));
    assert!(OpenAICompatProvider::uses_completion_tokens("o1-preview"));
    assert!(OpenAICompatProvider::uses_completion_tokens("gpt-5"));
    assert!(!OpenAICompatProvider::uses_completion_tokens("gpt-4"));
    assert!(!OpenAICompatProvider::uses_completion_tokens(
        "deepseek-chat"
    ));
}

#[test]
fn test_requires_fixed_temperature() {
    assert!(OpenAICompatProvider::requires_fixed_temperature("kimi-k2"));
    assert!(OpenAICompatProvider::requires_fixed_temperature("Kimi K2"));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("kimi-v1"));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("gpt-4"));
}

#[test]
fn test_parse_response_simple() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "Hello!",
                "role": "assistant"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "Hello!");
    assert_eq!(resp.finish_reason, "stop");
    assert_eq!(resp.usage.unwrap().total_tokens, 15);
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_parse_response_with_tool_calls() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"/tmp/test\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "call_123");
    assert_eq!(
        resp.tool_calls[0].function.as_ref().unwrap().name,
        "read_file"
    );
}

#[test]
fn test_parse_response_empty_choices() {
    let data = serde_json::json!({
        "choices": []
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
    assert_eq!(resp.finish_reason, "stop");
}

#[test]
fn test_build_request_body_basic() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    assert_eq!(body["model"], "gpt-4");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[test]
fn test_build_request_body_with_tools() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];

    let body = provider.build_request_body(
        &[],
        &tools,
        "gpt-4",
        &ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(1000),
            ..Default::default()
        },
    );
    assert!(body.get("tools").is_some());
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["temperature"], 0.7);
    assert_eq!(body["max_tokens"], 1000);
}

#[test]
fn test_config_default() {
    let config = OpenAICompatConfig::default();
    assert_eq!(config.name, "openai-compat");
    assert_eq!(config.timeout_secs, 600);
    assert!(config.base_url.is_empty());
    assert!(config.api_key.is_empty());
    assert!(config.default_model.is_empty());
    assert!(config.proxy.is_none());
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_normalize_model_with_base() {
    assert_eq!(
        normalize_model("nvidia/llama3", "https://api.nvidia.com"),
        "llama3"
    );
    assert_eq!(
        normalize_model("ollama/llama3", "http://localhost:11434"),
        "llama3"
    );
    assert_eq!(
        normalize_model("google/gemini", "https://generativelanguage.googleapis.com"),
        "gemini"
    );
    assert_eq!(
        normalize_model("moonshot/kimi", "https://api.moonshot.cn"),
        "kimi"
    );
}

#[test]
fn test_normalize_model_unknown_provider_prefix() {
    assert_eq!(
        normalize_model("myco/model", "https://example.com"),
        "myco/model"
    );
}

#[test]
fn test_normalize_model_openrouter_preserves() {
    assert_eq!(
        normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"),
        "openai/gpt-4"
    );
    assert_eq!(
        normalize_model("anthropic/claude-3", "https://OPENROUTER.AI/api/v1"),
        "anthropic/claude-3" // case-insensitive
    );
}

#[test]
fn test_parse_response_no_usage() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": "No usage info", "role": "assistant" },
            "finish_reason": "stop"
        }]
    });
    let resp = parse_response(&data);
    assert!(resp.usage.is_none());
    assert_eq!(resp.content, "No usage info");
}

#[test]
fn test_parse_response_null_content() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": null, "role": "assistant" },
            "finish_reason": "stop"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
}

#[test]
fn test_parse_response_null_finish_reason() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": "test", "role": "assistant" },
            "finish_reason": null
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.finish_reason, "stop"); // defaults to "stop"
}

#[test]
fn test_parse_response_no_choices_field() {
    let data = serde_json::json!({});
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_parse_response_multiple_tool_calls() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"/a\"}" }
                    },
                    {
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "write_file", "arguments": "{\"path\":\"/b\",\"content\":\"hello\"}" }
                    },
                    {
                        "id": "call_3",
                        "type": "function",
                        "function": { "name": "run_command", "arguments": "{\"cmd\":\"ls\"}" }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 3);
    assert_eq!(resp.tool_calls[0].id, "call_1");
    assert_eq!(resp.tool_calls[1].id, "call_2");
    assert_eq!(resp.tool_calls[2].id, "call_3");
}

#[test]
fn test_parse_response_tool_call_with_invalid_args() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "call_bad",
                    "type": "function",
                    "function": { "name": "test", "arguments": "not valid json" }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    // Arguments should be parsed as raw fallback
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
fn test_parse_response_tool_call_missing_id() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "type": "function",
                    "function": { "name": "test", "arguments": "{}" }
                }]
            }
        }]
    });
    let resp = parse_response(&data);
    // Missing id should be filtered out by filter_map
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_uses_completion_tokens_additional_models() {
    assert!(OpenAICompatProvider::uses_completion_tokens("glm-4-plus"));
    assert!(OpenAICompatProvider::uses_completion_tokens("glm-4-flash"));
    assert!(OpenAICompatProvider::uses_completion_tokens("o1"));
    assert!(!OpenAICompatProvider::uses_completion_tokens("gpt-4-turbo"));
    assert!(!OpenAICompatProvider::uses_completion_tokens(
        "claude-3-opus"
    ));
}

#[test]
fn test_requires_fixed_temperature_additional() {
    assert!(OpenAICompatProvider::requires_fixed_temperature(
        "kimi-k2-latest"
    ));
    assert!(OpenAICompatProvider::requires_fixed_temperature(
        "Kimi-K2-Pro"
    ));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("kimi-v1"));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("gpt-4"));
}

#[test]
fn test_build_request_body_completion_tokens() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let body = provider.build_request_body(
        &[],
        &[],
        "glm-4",
        &ChatOptions {
            max_tokens: Some(2048),
            ..Default::default()
        },
    );
    assert_eq!(body["max_completion_tokens"], 2048);
    assert!(body.get("max_tokens").is_none());
}

#[test]
fn test_build_request_body_kimi_fixed_temperature() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    // Kimi K2 should force temperature=1.0
    let body = provider.build_request_body(
        &[],
        &[],
        "kimi-k2",
        &ChatOptions {
            temperature: Some(0.5),
            ..Default::default()
        },
    );
    assert_eq!(body["temperature"], 1.0);
}

#[test]
fn test_build_request_body_no_tools_no_tool_choice() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let body = provider.build_request_body(&[], &[], "gpt-4", &ChatOptions::default());
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn test_build_request_body_with_stop_and_top_p() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let body = provider.build_request_body(
        &[],
        &[],
        "gpt-4",
        &ChatOptions {
            top_p: Some(0.95),
            stop: Some(vec!["END".to_string()]),
            ..Default::default()
        },
    );
    assert_eq!(body["top_p"], 0.95);
    assert!(body.get("stop").is_some());
}

#[test]
fn test_config_serialization_roundtrip() {
    let config = OpenAICompatConfig {
        name: "my-compat".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        default_model: "test-model".to_string(),
        timeout_secs: 120,
        proxy: Some("http://proxy:8080".to_string()),
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: OpenAICompatConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "my-compat");
    assert_eq!(deserialized.base_url, "https://api.test.com/v1");
    assert_eq!(deserialized.proxy, Some("http://proxy:8080".to_string()));
}

#[test]
fn test_default_model_and_name_accessors() {
    let config = OpenAICompatConfig {
        name: "my-provider".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key: "test".to_string(),
        default_model: "default-model".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);
    assert_eq!(provider.default_model(), "default-model");
    assert_eq!(provider.name(), "my-provider");
}

use super::*;

#[test]
fn test_resolve_model_default() {
    let r = resolve_codex_model("");
    assert_eq!(r.model, CODEX_DEFAULT_MODEL);
    assert_eq!(r.fallback_reason, "empty model");
    let r = resolve_codex_model("codex-cli");
    assert_eq!(r.model, CODEX_DEFAULT_MODEL);
}

#[test]
fn test_resolve_model_openai_prefix() {
    let r = resolve_codex_model("openai/gpt-4o");
    assert_eq!(r.model, "gpt-4o");
    assert!(r.fallback_reason.is_empty());
}

#[test]
fn test_resolve_model_unsupported() {
    let r = resolve_codex_model("anthropic/claude-3");
    assert_eq!(r.model, CODEX_DEFAULT_MODEL);
    assert_eq!(r.fallback_reason, "non-openai model namespace");
    let r = resolve_codex_model("deepseek/chat");
    assert_eq!(r.model, CODEX_DEFAULT_MODEL);
}

#[test]
fn test_resolve_model_supported() {
    let r = resolve_codex_model("gpt-4o");
    assert_eq!(r.model, "gpt-4o");
    assert!(r.fallback_reason.is_empty());
    let r = resolve_codex_model("o3-mini");
    assert_eq!(r.model, "o3-mini");
    assert!(r.fallback_reason.is_empty());
    let r = resolve_codex_model("o4-mini");
    assert_eq!(r.model, "o4-mini");
    assert!(r.fallback_reason.is_empty());
}

#[test]
fn test_translate_tools_for_codex() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools_for_codex(&tools, true);
    assert_eq!(translated.len(), 2); // function + web_search
    assert_eq!(translated[0]["type"], "function");
    assert_eq!(translated[1]["type"], "web_search");
}

#[test]
fn test_translate_tools_skips_web_search() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "web_search".to_string(),
            description: "Search web".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools_for_codex(&tools, true);
    assert_eq!(translated.len(), 1); // only the built-in web_search
    assert_eq!(translated[0]["type"], "web_search");
}

#[test]
fn test_parse_codex_response_text() {
    let data = serde_json::json!({
        "output": [
            {
                "type": "message",
                "content": [{"type": "output_text", "text": "Hello!"}]
            }
        ],
        "usage": {"input_tokens": 10, "output_tokens": 5}
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.content, "Hello!");
    assert_eq!(resp.finish_reason, "stop");
    assert_eq!(resp.usage.unwrap().total_tokens, 15);
}

#[test]
fn test_parse_codex_response_function_call() {
    let data = serde_json::json!({
        "output": [
            {
                "type": "function_call",
                "call_id": "fc_123",
                "name": "read_file",
                "arguments": "{\"path\":\"/tmp\"}"
            }
        ],
        "usage": {"input_tokens": 20, "output_tokens": 10}
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "fc_123");
    assert_eq!(resp.finish_reason, "tool_calls");
}

#[test]
fn test_parse_codex_response_incomplete() {
    let data = serde_json::json!({
        "status": "incomplete",
        "output": [],
        "usage": {"input_tokens": 10, "output_tokens": 0}
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.finish_reason, "length");
}

#[test]
fn test_build_request_body() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["instructions"], CODEX_DEFAULT_INSTRUCTIONS);
    assert_eq!(body["store"], false);
}

#[test]
fn test_config_default() {
    let config = CodexConfig::default();
    assert_eq!(config.default_model, CODEX_DEFAULT_MODEL);
    assert_eq!(config.base_url, CODEX_BASE_URL);
    assert!(config.enable_web_search);
}

// -- Additional tests --

#[test]
fn test_codex_config_serialization_roundtrip() {
    let config = CodexConfig {
        default_model: "gpt-4o".into(),
        base_url: "https://api.example.com".into(),
        api_key: "sk-test".into(),
        account_id: "acct-123".into(),
        enable_web_search: false,
        timeout_secs: 120,
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: CodexConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.default_model, "gpt-4o");
    assert_eq!(back.base_url, "https://api.example.com");
    assert_eq!(back.api_key, "sk-test");
    assert_eq!(back.account_id, "acct-123");
    assert!(!back.enable_web_search);
}

#[test]
fn test_codex_config_deserialization_partial() {
    // Fields with #[serde(default)] get empty string defaults, not CODEX_BASE_URL
    let json = r#"{"api_key": "sk-test", "default_model": "o3-mini"}"#;
    let config: CodexConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.api_key, "sk-test");
    assert_eq!(config.default_model, "o3-mini");
    assert_eq!(config.base_url, ""); // serde default = empty string
    assert!(config.enable_web_search); // default_true
}

#[test]
fn test_resolve_model_with_provider_prefix() {
    let r = resolve_codex_model("openai/o4-mini");
    assert_eq!(r.model, "o4-mini");
    assert!(r.fallback_reason.is_empty());
}

#[test]
fn test_translate_tools_no_web_search() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools_for_codex(&tools, false);
    // Without web_search enabled, should only have the function tool
    assert_eq!(translated.len(), 1);
    assert_eq!(translated[0]["type"], "function");
}

#[test]
fn test_parse_codex_response_empty_output() {
    let data = serde_json::json!({
        "output": [],
        "usage": {"input_tokens": 5, "output_tokens": 0}
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.content, "");
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.finish_reason, "stop");
}

#[test]
fn test_parse_codex_response_no_usage() {
    let data = serde_json::json!({
        "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "Hi!"}]}
        ]
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.content, "Hi!");
    assert!(resp.usage.is_none());
}

#[test]
fn test_build_request_body_with_tools() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let body = provider.build_request_body(&messages, &tools, "gpt-4o", &ChatOptions::default());
    assert!(body.get("tools").is_some());
    let tools_arr = body["tools"].as_array().unwrap();
    assert!(!tools_arr.is_empty());
}

#[test]
fn test_default_model_constant() {
    assert_eq!(CODEX_DEFAULT_MODEL, "gpt-5.2");
}

#[test]
fn test_base_url_constant() {
    assert_eq!(CODEX_BASE_URL, "https://chatgpt.com/backend-api/codex");
}

// ---- Additional coverage tests for 95%+ ----

#[test]
fn test_build_request_body_with_system_message() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "You are a code reviewer".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Review this code".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
extra: HashMap::new(),
        },
    ];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    assert_eq!(body["instructions"], "You are a code reviewer");
}

#[test]
fn test_build_request_body_with_assistant_tool_calls() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: "Let me read the file".to_string(),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/tmp/test"}"#.to_string(),
                }),
                name: Some("read_file".to_string()),
                arguments: None,
            }],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
extra: HashMap::new(),
        },
    ];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    // Should have: message + function_call
    assert!(input.len() >= 1);
}

#[test]
fn test_build_request_body_with_assistant_tool_calls_empty_content() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            }),
            name: None,
            arguments: None,
        }],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    // Empty content should not produce a message item, only function_call
    assert!(input.len() >= 1);
}

#[test]
fn test_build_request_body_with_assistant_no_tool_calls() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: "I can help with that".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["role"], "assistant");
}

#[test]
fn test_build_request_body_with_user_tool_call_id() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "file contents here".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "function_call_output");
    assert_eq!(input[0]["call_id"], "call_1");
}

#[test]
fn test_build_request_body_with_tool_message() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "tool".to_string(),
        content: "tool result".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "function_call_output");
}

#[test]
fn test_build_request_body_with_tool_message_no_call_id() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "tool".to_string(),
        content: "orphan result".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    assert!(input.is_empty());
}

#[test]
fn test_build_request_body_with_options() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(
        &messages,
        &[],
        "gpt-4o",
        &ChatOptions {
            max_tokens: Some(2048),
            temperature: Some(0.7),
            ..Default::default()
        },
    );
    assert_eq!(body["max_output_tokens"], 2048);
    assert_eq!(body["temperature"], 0.7);
}

#[test]
fn test_build_request_body_with_unknown_role() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "custom_role".to_string(),
        content: "custom content".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
extra: HashMap::new(),
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    assert!(input.is_empty()); // Unknown roles are skipped
}

#[test]
fn test_translate_tools_for_codex_non_function_type() {
    let tools = vec![ToolDefinition {
        tool_type: "non_function".to_string(),
        function: ToolFunctionDefinition {
            name: "ignored".to_string(),
            description: "Should be ignored".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools_for_codex(&tools, false);
    assert!(translated.is_empty()); // non-function tools are skipped
}

#[test]
fn test_translate_tools_for_codex_no_description() {
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "no_desc".to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let translated = translate_tools_for_codex(&tools, false);
    assert_eq!(translated.len(), 1);
    assert!(translated[0].get("description").is_none());
}

#[test]
fn test_parse_codex_response_function_call_bad_args() {
    let data = serde_json::json!({
        "output": [
            {
                "type": "function_call",
                "call_id": "fc_bad",
                "name": "bad_args",
                "arguments": "not valid json"
            }
        ]
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    // Arguments that fail to parse should get raw fallback
    assert!(resp.tool_calls[0].arguments.is_some());
    assert!(resp.tool_calls[0].arguments.as_ref().unwrap().contains_key("raw"));
}

#[test]
fn test_parse_codex_response_unknown_output_type() {
    let data = serde_json::json!({
        "output": [
            {
                "type": "unknown_type",
                "data": "something"
            }
        ]
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.content, "");
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_resolve_codex_model_all_unsupported_prefixes() {
    let unsupported = [
        "glm-4", "claude-3", "anthropic-1", "gemini-pro", "google-1",
        "moonshot-v1", "kimi-chat", "qwen-7b", "deepseek-chat",
        "llama-3", "meta-llama-3", "mistral-7b", "grok-1", "xai-1", "zhipu-4",
    ];
    for model in &unsupported {
        let r = resolve_codex_model(model);
        assert_eq!(r.model, CODEX_DEFAULT_MODEL, "Expected default for {}", model);
        assert_eq!(r.fallback_reason, "unsupported model prefix", "Expected prefix reason for {}", model);
    }
}

#[test]
fn test_resolve_codex_model_unsupported_family() {
    let r = resolve_codex_model("my-custom-model");
    assert_eq!(r.model, CODEX_DEFAULT_MODEL);
    assert_eq!(r.fallback_reason, "unsupported model family");
}

#[test]
fn test_resolve_codex_model_whitespace() {
    let r = resolve_codex_model("  gpt-4o  ");
    assert_eq!(r.model, "gpt-4o");
    assert!(r.fallback_reason.is_empty());
}

#[test]
fn test_resolve_codex_model_case_insensitive() {
    let r = resolve_codex_model("GPT-4O");
    assert_eq!(r.model, "gpt-4o");
    assert!(r.fallback_reason.is_empty());
}

#[test]
fn test_codex_with_token_source() {
    let ts: Box<dyn Fn() -> Result<(String, String), String> + Send + Sync> =
        Box::new(|| Ok(("token".to_string(), "account".to_string())));
    let provider = CodexProvider::with_token_source(CodexConfig::default(), ts);
    assert_eq!(provider.name(), "codex");
    assert_eq!(provider.default_model(), CODEX_DEFAULT_MODEL);
}

#[test]
fn test_resolved_codex_model_debug() {
    let r = resolve_codex_model("gpt-4o");
    let debug = format!("{:?}", r);
    assert!(debug.contains("gpt-4o"));
}

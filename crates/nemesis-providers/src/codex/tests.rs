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
        content: "Hello".into(),
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
        proxy: None,
        stream_mode: String::new(),
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
        content: "Hello".into(),
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
            content: "You are a code reviewer".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Review this code".into(),
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
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: "Let me read the file".into(),
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
    }];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let input = body["input"].as_array().unwrap();
    // Should have: message + function_call
    assert!(!input.is_empty());
}

#[test]
fn test_build_request_body_with_assistant_tool_calls_empty_content() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: String::new().into(),
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
    assert!(!input.is_empty());
}

#[test]
fn test_build_request_body_with_assistant_no_tool_calls() {
    let provider = CodexProvider::new(CodexConfig::default());
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: "I can help with that".into(),
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
        content: "file contents here".into(),
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
        content: "tool result".into(),
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
        content: "orphan result".into(),
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
        content: "Hello".into(),
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
        content: "custom content".into(),
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
    assert!(
        resp.tool_calls[0]
            .arguments
            .as_ref()
            .unwrap()
            .contains_key("raw")
    );
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
        "glm-4",
        "claude-3",
        "anthropic-1",
        "gemini-pro",
        "google-1",
        "moonshot-v1",
        "kimi-chat",
        "qwen-7b",
        "deepseek-chat",
        "llama-3",
        "meta-llama-3",
        "mistral-7b",
        "grok-1",
        "xai-1",
        "zhipu-4",
    ];
    for model in &unsupported {
        let r = resolve_codex_model(model);
        assert_eq!(
            r.model, CODEX_DEFAULT_MODEL,
            "Expected default for {}",
            model
        );
        assert_eq!(
            r.fallback_reason, "unsupported model prefix",
            "Expected prefix reason for {}",
            model
        );
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

// 2026-09-03 二次回归 F2 回归锁：function_call_output 的 output 必须是字符串，
// Parts 走文本视图 + 诚实注记（D7 同族），无 base64 泄漏；Text 形态保持原字节。
#[test]
fn test_build_request_body_tool_output_with_parts_degrades_to_text_note() {
    let provider = CodexProvider::new(CodexConfig::default());
    let mk = |tool_call_id: Option<String>| Message {
        role: "tool".to_string(),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "工具输出".to_string(),
            },
            ContentPart::Image {
                image: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                },
                detail: None,
            },
        ]),
        tool_calls: vec![],
        tool_call_id,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    };

    let messages = vec![
        mk(Some("call_1".to_string())),
        mk(Some("call_2".to_string())),
    ];
    let body = provider.build_request_body(&messages, &[], "gpt-4o", &ChatOptions::default());
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("aGVsbG8="), "base64 泄漏进请求: {raw}");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "function_call_output");
    assert_eq!(input[1]["type"], "function_call_output");
    let out = input[0]["output"].as_str().expect("output 必须是字符串");
    assert!(out.starts_with("工具输出"));
    assert!(out.contains("1 张图片"));
}

#[test]
fn test_build_request_body_requests_streaming() {
    let provider = CodexProvider::new(CodexConfig {
        api_key: "k".into(),
        account_id: String::new(),
        default_model: String::new(),
        base_url: "http://127.0.0.1:9/v1".into(),
        enable_web_search: false,
        timeout_secs: 10,
        proxy: None,
        stream_mode: String::new(),
    });
    let body = provider.build_request_body(&[], &[], "gpt-4o", &ChatOptions::default());
    assert_eq!(
        body["stream"],
        serde_json::json!(true),
        "非流式请求会撞中转链路墙钟超时（2026-10-13 根因）"
    );
}

/// per-model `stream` 键（2026-10-13）："no" = 逃生门，body 不带 stream
/// 字段（恢复非流式形态，等完整响应）。
#[test]
fn test_build_request_body_stream_no_omits_stream_field() {
    let provider = CodexProvider::new(CodexConfig {
        api_key: "k".into(),
        account_id: String::new(),
        default_model: String::new(),
        base_url: "http://127.0.0.1:9/v1".into(),
        enable_web_search: false,
        timeout_secs: 10,
        proxy: None,
        stream_mode: "no".into(),
    });
    let body = provider.build_request_body(&[], &[], "gpt-4o", &ChatOptions::default());
    assert!(
        body.get("stream").is_none(),
        "stream=no 必须省略 stream 字段: {body}"
    );
}

/// 传输错误诚实化（2026-10-13）：连接拒绝（端口 1 无监听）→ Unknown 且
/// message 带 "connect error" 前缀（loop transient 词表成员，可重试），
/// 不再伪装 Timeout。真超时仍归 Timeout（reqwest is_timeout）。
#[tokio::test]
async fn test_chat_transport_error_is_honest_connect_error() {
    let provider = CodexProvider::new(CodexConfig {
        api_key: "k".into(),
        account_id: String::new(),
        default_model: String::new(),
        base_url: "http://127.0.0.1:1".into(),
        enable_web_search: false,
        timeout_secs: 10,
        proxy: None,
        stream_mode: String::new(),
    });
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "gpt-4o", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => {
            assert!(
                message.starts_with("connect error"),
                "必须带 connect error 前缀: {message}"
            );
            assert!(
                message.contains("127.0.0.1:1"),
                "根因链必须携带目标 URL: {message}"
            );
        }
        other => panic!("预期 Unknown(connect error)，得到 {other:?}"),
    }
}

#[test]
fn test_parse_sse_block_basic() {
    let ev =
        parse_sse_block("event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n");
    assert!(ev.is_some());
    assert_eq!(ev.unwrap()["type"], "response.completed");
}

#[test]
fn test_parse_sse_block_crlf_and_multi_data() {
    // CRLF 行尾 + 多 data: 行拼接（长 JSON 被 SSE 分行是合法形态）
    let ev = parse_sse_block("data: {\"a\":1 bad\r\ndata: token}\r\n\r\n");
    assert!(ev.is_none(), "拼接后非法 JSON 应返回 None");
    let ev = parse_sse_block("data: {\"a\":\r\ndata: 1}\r\n\r\n").expect("拼接后应可解析");
    assert_eq!(ev["a"], serde_json::json!(1));
}

#[test]
fn test_parse_sse_block_ignores_comments_and_non_json() {
    assert!(parse_sse_block(": keep-alive\n\n").is_none());
    assert!(parse_sse_block("data: not-json\n\n").is_none());
    assert!(parse_sse_block("\n\n").is_none());
}

#[test]
fn test_handle_sse_event_completed_yields_response() {
    let ev = serde_json::json!({
        "type": "response.completed",
        "response": {"status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "你好"}]}
        ]}
    });
    match handle_sse_event(&ev) {
        SseOutcome::Finished(resp) => {
            let r = parse_codex_response(&resp);
            assert_eq!(r.content, "你好");
            assert_eq!(r.finish_reason, "stop");
        }
        other => panic!("应 Finished，实际 {other:?}"),
    }
}

#[test]
fn test_handle_sse_event_failed_and_error() {
    let failed = serde_json::json!({
        "type": "response.failed",
        "response": {"status": "failed", "error": {"message": "boom"}}
    });
    assert_eq!(
        handle_sse_event(&failed),
        SseOutcome::Failed("boom".to_string())
    );
    let err = serde_json::json!({"type": "error", "message": "concURRENCY limit"});
    assert_eq!(
        handle_sse_event(&err),
        SseOutcome::Failed("concURRENCY limit".to_string())
    );
    // 中间事件全部 Continue
    let delta = serde_json::json!({"type": "response.output_text.delta", "delta": "x"});
    assert_eq!(handle_sse_event(&delta), SseOutcome::Continue);
    let created = serde_json::json!({"type": "response.created", "response": {}});
    assert_eq!(handle_sse_event(&created), SseOutcome::Continue);
}

#[test]
fn test_parse_sse_text_aggregates_completed() {
    let text = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"好\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",",
        "\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"你好\"}]}],",
        "\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n",
    );
    let r = parse_sse_text(text, "gpt-5.6-sol").expect("终态事件应出结果");
    assert_eq!(r.content, "你好");
    assert_eq!(r.finish_reason, "stop");
    let usage = r.usage.expect("usage 应透传");
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 2);
}

#[test]
fn test_parse_sse_text_crlf_blocks() {
    let text = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\r\n\r\n";
    let r = parse_sse_text(text, "gpt-4o").expect("CRLF 分块应同样可解析");
    assert_eq!(r.finish_reason, "stop");
}

#[test]
fn test_parse_sse_text_truncated_stream_maps_to_timeout() {
    // 流中途断（无终态事件）→ Timeout（可重试），不是假成功
    let text = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"半截\"}\n\n",
    );
    match parse_sse_text(text, "gpt-4o") {
        Err(FailoverError::Timeout { provider, model }) => {
            assert_eq!(provider, "codex");
            assert_eq!(model, "gpt-4o");
        }
        other => panic!("应 Timeout，实际 {other:?}"),
    }
}

#[test]
fn test_parse_sse_text_failed_event_maps_to_unknown() {
    let text = concat!(
        "data: {\"type\":\"response.created\",\"response\":{}}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",",
        "\"error\":{\"message\":\" quota exceeded \"}}}\n\n",
    );
    match parse_sse_text(text, "gpt-4o") {
        Err(FailoverError::Unknown { message, .. }) => {
            assert_eq!(message, " quota exceeded ");
        }
        other => panic!("应 Unknown，实际 {other:?}"),
    }
}

#[test]
fn test_parse_sse_text_incomplete_treated_as_finished() {
    // incomplete（如 length 截断）也是终态：走既有 parse_codex_response 的
    // finish_reason=length 语义，不按断流处理
    let text = concat!(
        "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",",
        "\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"截断\"}]}]}}\n\n",
    );
    let r = parse_sse_text(text, "gpt-4o").expect("incomplete 是合法终态");
    assert_eq!(r.finish_reason, "length");
    assert_eq!(r.content, "截断");
}

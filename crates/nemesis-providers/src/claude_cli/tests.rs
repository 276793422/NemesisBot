use super::*;

#[test]
fn test_build_system_prompt() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![Message {
        role: "system".to_string(),
        content: "You are helpful".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let prompt = provider.build_system_prompt(&messages, &[]);
    assert_eq!(prompt, "You are helpful");
}

#[test]
fn test_build_tools_prompt() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let prompt = provider.build_tools_prompt(&tools);
    assert!(prompt.contains("Available Tools"));
    assert!(prompt.contains("read_file"));
}

#[test]
fn test_messages_to_prompt_single_user() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello world".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let prompt = provider.messages_to_prompt(&messages);
    assert_eq!(prompt, "Hello world");
}

#[test]
fn test_messages_to_prompt_multi() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: "Hello".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "assistant".to_string(),
            content: "Hi there".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
    ];
    let prompt = provider.messages_to_prompt(&messages);
    assert!(prompt.contains("User: Hello"));
    assert!(prompt.contains("Assistant: Hi there"));
}

#[test]
fn test_parse_response_text() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":false,"result":"Hello!","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}"#;
    let resp = provider.parse_response(output, &[]).unwrap();
    assert_eq!(resp.content, "Hello!");
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.usage.unwrap().total_tokens, 15);
}

#[test]
fn test_parse_response_error() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":true,"result":"Something went wrong","usage":{}}"#;
    let result = provider.parse_response(output, &[]);
    assert!(result.is_err());
}

#[test]
fn test_parse_response_with_tool_calls() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":false,"result":"Using tool {\"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"/tmp\\\"}\"}}]}","usage":{"input_tokens":20,"output_tokens":10}}"#;
    // 白名单内名字（read_file）→ 正常恢复为工具调用（BUG-20261103-338 过滤
    // 只拦名字不可执行的伪调用，不碰机制本身）。
    let tools = vec![fn_tool("read_file")];
    let resp = provider.parse_response(output, &tools).unwrap();
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(
        resp.tool_calls[0].function.as_ref().unwrap().name,
        "read_file"
    );
    assert_eq!(resp.finish_reason, "tool_calls");
    assert!(!resp.content.contains("tool_calls"));
}

#[test]
fn test_config_default() {
    let config = ClaudeCliConfig::default();
    assert_eq!(config.command, "claude");
    assert_eq!(config.default_model, "claude-code");
}

// -- Additional tests --

#[test]
fn test_claude_cli_config_serialization_roundtrip() {
    let config = ClaudeCliConfig {
        command: "custom-claude".into(),
        workspace: "/tmp/project".into(),
        default_model: "claude-3-opus".into(),
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: ClaudeCliConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.command, "custom-claude");
    assert_eq!(back.workspace, "/tmp/project");
    assert_eq!(back.default_model, "claude-3-opus");
}

#[test]
fn test_claude_cli_config_deserialization_defaults() {
    let json = r#"{}"#;
    let config: ClaudeCliConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.command, "claude");
    assert!(config.workspace.is_empty());
    assert!(config.default_model.is_empty());
}

#[test]
fn test_build_system_prompt_with_tools() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![Message {
        role: "system".to_string(),
        content: "Be helpful".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let prompt = provider.build_system_prompt(&messages, &tools);
    assert!(prompt.contains("Be helpful"));
    assert!(prompt.contains("Available Tools"));
    assert!(prompt.contains("read_file"));
}

#[test]
fn test_build_system_prompt_no_system_messages() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let prompt = provider.build_system_prompt(&messages, &[]);
    assert!(prompt.is_empty());
}

#[test]
fn test_messages_to_prompt_with_tool_result() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: "Check".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "tool".to_string(),
            content: "result data".into(),
            tool_calls: vec![],
            tool_call_id: Some("call_1".into()),
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
    ];
    let prompt = provider.messages_to_prompt(&messages);
    assert!(prompt.contains("[Tool Result for call_1]: result data"));
}

#[test]
fn test_messages_to_prompt_system_ignored() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "System prompt".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hello".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
    ];
    let prompt = provider.messages_to_prompt(&messages);
    // System messages should not appear in the prompt (handled by --system-prompt flag)
    assert!(!prompt.contains("System prompt"));
    assert_eq!(prompt, "Hello"); // single user message simplified
}

#[test]
fn test_messages_to_prompt_tool_without_call_id() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![Message {
        role: "tool".to_string(),
        content: "orphan result".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let prompt = provider.messages_to_prompt(&messages);
    // Tool message without tool_call_id should be skipped
    assert!(prompt.is_empty());
}

#[test]
fn test_parse_response_with_usage() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":false,"result":"Done!","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}"#;
    let resp = provider.parse_response(output, &[]).unwrap();
    assert_eq!(resp.content, "Done!");
    let usage = resp.usage.unwrap();
    // Anthropic 加法语义透传：prompt=纯输入 100，缓存读写单列；
    // total 按账本口径还原全量（100+10+20+50）。
    assert_eq!(usage.prompt_tokens, 100);
    assert_eq!(usage.completion_tokens, 50);
    assert_eq!(usage.total_tokens, 180);
    assert_eq!(usage.cached_tokens, Some(20));
    assert_eq!(usage.cache_creation_tokens, Some(10));
    assert_eq!(usage.cache_read_tokens, Some(20));
}

#[test]
fn test_parse_response_no_usage() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":false,"result":"No usage info","usage":{}}"#;
    let resp = provider.parse_response(output, &[]).unwrap();
    assert_eq!(resp.content, "No usage info");
    assert!(resp.usage.is_none()); // Both input and output are 0
}

#[test]
fn test_parse_response_invalid_json() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let result = provider.parse_response("not json at all", &[]);
    assert!(result.is_err());
}

#[test]
fn test_provider_name_and_default_model() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    assert_eq!(provider.name(), "claude-cli");
    assert_eq!(provider.default_model(), "claude-code");
}

// ============================================================
// T3：CLI 委派降级（D7）——图像 → 文字占位，不静默
// ============================================================

#[test]
fn test_messages_to_prompt_image_degrades_to_note() {
    let config = crate::claude_cli::ClaudeCliConfig::default();
    let provider = crate::claude_cli::ClaudeCliProvider::new(config);
    let messages = vec![
        Message::text("user", "看看这张图"),
        Message::parts(
            "user",
            vec![
                ContentPart::Text {
                    text: "第二张".to_string(),
                },
                ContentPart::Image {
                    image: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: "aGVsbG8=".to_string(),
                    },
                    detail: None,
                },
            ],
        ),
    ];
    let prompt = provider.messages_to_prompt(&messages);
    assert!(prompt.contains("看看这张图"));
    assert!(prompt.contains("第二张"));
    assert!(prompt.contains("[用户附带 1 张图片（当前委派通道不支持视觉输入）]"));
    // 不泄露 base64
    assert!(!prompt.contains("aGVsbG8="));
}

// ============================================================
// BUG-20261103-338：文档性 `{"tool_calls"` JSON 不得劫持成幻影工具调用
// （BUG-20261025-16 同款白名单不变量）。文本信封是 CLI lane 唯一工具
// 调用通道，过滤只拦名字不可执行的伪调用，不碰机制本身。
// ============================================================

fn fn_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }
}

/// 文档性 JSON（非白名单名字）→ 不劫持：finish 维持 stop、正文原文保留。
#[test]
fn test_parse_response_documentation_json_not_hijacked() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let docs = "工具调用信封示例：\n{\"tool_calls\":[{\"id\":\"demo\",\"type\":\"function\",\"function\":{\"name\":\"not_a_real_tool\",\"arguments\":\"{}\"}}]}";
    let output = serde_json::json!({
        "type": "result",
        "is_error": false,
        "result": docs,
        "usage": {}
    })
    .to_string();
    let resp = provider
        .parse_response(&output, &[fn_tool("read_file")])
        .unwrap();
    assert!(resp.tool_calls.is_empty(), "{:?}", resp.tool_calls);
    assert_eq!(resp.finish_reason, "stop");
    // 文档原文保留（正文剥离只应发生在真恢复时）
    assert!(resp.content.contains("not_a_real_tool"), "{}", resp.content);
}

/// 空白名单（请求未提供任何工具 = 未教信封协议）→ 信封文本一律不劫持。
#[test]
fn test_parse_response_empty_tools_drops_envelope() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let docs = "信封长这样：{\"tool_calls\":[{\"id\":\"demo\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}}]}";
    let output = serde_json::json!({
        "type": "result",
        "is_error": false,
        "result": docs,
        "usage": {}
    })
    .to_string();
    let resp = provider.parse_response(&output, &[]).unwrap();
    assert!(resp.tool_calls.is_empty(), "{:?}", resp.tool_calls);
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.content.contains("tool_calls"), "{}", resp.content);
}

/// 白名单内名字混同非白名单名字 → 只保留可执行的，其余丢弃。
#[test]
fn test_parse_response_mixed_names_filters_to_whitelist() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let docs = "{\"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}},{\"id\":\"c2\",\"type\":\"function\",\"function\":{\"name\":\"ghost_tool\",\"arguments\":\"{}\"}}]}";
    let output = serde_json::json!({
        "type": "result",
        "is_error": false,
        "result": docs,
        "usage": {}
    })
    .to_string();
    let resp = provider
        .parse_response(&output, &[fn_tool("read_file")])
        .unwrap();
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "c1");
    assert_eq!(resp.finish_reason, "tool_calls");
}

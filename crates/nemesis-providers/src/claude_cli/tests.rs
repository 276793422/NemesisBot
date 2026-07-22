use super::*;

#[test]
fn test_build_system_prompt() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let messages = vec![Message {
        role: "system".to_string(),
        content: "You are helpful".to_string(),
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
        content: "Hello world".to_string(),
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
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "assistant".to_string(),
            content: "Hi there".to_string(),
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
    let resp = provider.parse_response(output).unwrap();
    assert_eq!(resp.content, "Hello!");
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.usage.unwrap().total_tokens, 15);
}

#[test]
fn test_parse_response_error() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":true,"result":"Something went wrong","usage":{}}"#;
    let result = provider.parse_response(output);
    assert!(result.is_err());
}

#[test]
fn test_parse_response_with_tool_calls() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":false,"result":"Using tool {\"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"/tmp\\\"}\"}}]}","usage":{"input_tokens":20,"output_tokens":10}}"#;
    // Note: this test uses the extract_tool_calls_from_text internally
    // The result field has escaped JSON which should be parsed by the extract function
    let _resp = provider.parse_response(output).unwrap();
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
        content: "Be helpful".to_string(),
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
        content: "Hello".to_string(),
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
            content: "Check".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "tool".to_string(),
            content: "result data".to_string(),
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
            content: "System prompt".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
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
        content: "orphan result".to_string(),
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
    let resp = provider.parse_response(output).unwrap();
    assert_eq!(resp.content, "Done!");
    let usage = resp.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 130); // 100 + 10 + 20
    assert_eq!(usage.completion_tokens, 50);
    assert_eq!(usage.total_tokens, 180);
}

#[test]
fn test_parse_response_no_usage() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let output = r#"{"type":"result","is_error":false,"result":"No usage info","usage":{}}"#;
    let resp = provider.parse_response(output).unwrap();
    assert_eq!(resp.content, "No usage info");
    assert!(resp.usage.is_none()); // Both input and output are 0
}

#[test]
fn test_parse_response_invalid_json() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    let result = provider.parse_response("not json at all");
    assert!(result.is_err());
}

#[test]
fn test_provider_name_and_default_model() {
    let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
    assert_eq!(provider.name(), "claude-cli");
    assert_eq!(provider.default_model(), "claude-code");
}

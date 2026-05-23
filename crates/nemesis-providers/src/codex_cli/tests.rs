use super::*;

    #[test]
    fn test_build_prompt_single_user() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
    extra: std::collections::HashMap::new(),
        }];
        let prompt = provider.build_prompt(&messages, &[]);
        assert_eq!(prompt, "Hello");
    }

    #[test]
    fn test_build_prompt_with_system() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "Be helpful".to_string(),
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
        let prompt = provider.build_prompt(&messages, &[]);
        assert!(prompt.contains("System Instructions"));
        assert!(prompt.contains("Be helpful"));
    }

    #[test]
    fn test_build_tools_prompt() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn test_parse_jsonl_agent_message() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Hello!"}}
{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.usage.unwrap().total_tokens, 17);
    }

    #[test]
    fn test_parse_jsonl_error_only() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"error","message":"something went wrong"}"#;
        let result = provider.parse_jsonl_events(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jsonl_error_with_content() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Partial result"}}
{"type":"turn.failed","error":{"message":"api error"}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "Partial result"); // content wins over error
    }

    #[test]
    fn test_parse_jsonl_empty() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let resp = provider.parse_jsonl_events("").unwrap();
        assert_eq!(resp.content, "");
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_jsonl_malformed_lines_skipped() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = "not json\n{\"type\":\"item.completed\",\"item\":{\"id\":\"i1\",\"type\":\"agent_message\",\"text\":\"OK\"}}\nalso not json";
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "OK");
    }

    #[test]
    fn test_config_default() {
        let config = CodexCliConfig::default();
        assert_eq!(config.command, "codex");
        assert_eq!(config.default_model, "codex-cli");
    }

    // -- Additional tests --

    #[test]
    fn test_codex_cli_config_serialization_roundtrip() {
        let config = CodexCliConfig {
            command: "custom-codex".into(),
            workspace: "/tmp/workspace".into(),
            default_model: "o3-mini".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: CodexCliConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command, "custom-codex");
        assert_eq!(back.workspace, "/tmp/workspace");
        assert_eq!(back.default_model, "o3-mini");
    }

    #[test]
    fn test_codex_cli_config_deserialization_defaults() {
        let json = r#"{}"#;
        let config: CodexCliConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "codex"); // default via serde
        assert!(config.workspace.is_empty());
        assert!(config.default_model.is_empty());
    }

    #[test]
    fn test_build_prompt_with_tools_and_system() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "Be helpful".to_string(),
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
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "calc".to_string(),
                description: "Calculate".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_prompt(&messages, &tools);
        assert!(prompt.contains("System Instructions"));
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("calc"));
        assert!(prompt.contains("Hello"));
    }

    #[test]
    fn test_build_prompt_with_assistant_message() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Hi".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
    extra: std::collections::HashMap::new(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Hello!".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
    extra: std::collections::HashMap::new(),
            },
        ];
        let prompt = provider.build_prompt(&messages, &[]);
        assert!(prompt.contains("Assistant: Hello!"));
        assert!(prompt.contains("Hi"));
    }

    #[test]
    fn test_build_prompt_with_tool_result() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Read file".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
    extra: std::collections::HashMap::new(),
            },
            Message {
                role: "tool".to_string(),
                content: "file content here".to_string(),
                tool_calls: vec![],
                tool_call_id: Some("call_123".into()),
                timestamp: None,
                reasoning_content: None,
    extra: std::collections::HashMap::new(),
            },
        ];
        let prompt = provider.build_prompt(&messages, &[]);
        assert!(prompt.contains("[Tool Result for call_123]: file content here"));
    }

    #[test]
    fn test_build_tools_prompt_non_function_type() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "non_function".to_string(),
            function: ToolFunctionDefinition {
                name: "ignored".to_string(),
                description: "Should be ignored".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        // Non-function tools should be skipped
        assert!(prompt.contains("Available Tools"));
        assert!(!prompt.contains("ignored"));
    }

    #[test]
    fn test_build_tools_prompt_no_params() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "simple".to_string(),
                description: "Simple tool".to_string(),
                parameters: serde_json::Value::Null,
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        assert!(prompt.contains("simple"));
        assert!(prompt.contains("Simple tool"));
        // Should not contain Parameters section when params is null
        assert!(!prompt.contains("Parameters:\n"));
    }

    #[test]
    fn test_parse_jsonl_turn_completed_no_usage() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Hello!"}}
{"type":"turn.completed"}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "Hello!");
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_parse_jsonl_turn_failed_no_error() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"turn.failed"}"#;
        // turn.failed without error field, no content - returns empty response (not error)
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_jsonl_non_agent_message_item() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"tool_call","text":"ignored"}}
{"type":"turn.completed","usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":2}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        // Non agent_message items are skipped
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_jsonl_item_completed_empty_text() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":""}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        // Empty text is skipped
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_jsonl_cached_tokens_in_usage() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":5,"output_tokens":3}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 15); // 10 + 5 cached
        assert_eq!(usage.completion_tokens, 3);
        assert_eq!(usage.total_tokens, 18);
    }

    #[test]
    fn test_provider_name_and_default_model() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        assert_eq!(provider.name(), "codex-cli");
        assert_eq!(provider.default_model(), "codex-cli");
    }

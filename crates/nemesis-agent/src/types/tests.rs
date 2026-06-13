use super::*;

#[test]
fn agent_config_default() {
    let config = AgentConfig::default();
    assert_eq!(config.model, "gpt-4");
    assert!(config.system_prompt.is_none());
    assert_eq!(config.max_turns, 10);
    assert!(config.tools.is_empty());
}

#[test]
fn agent_config_serialization_roundtrip() {
    let config = AgentConfig {
        model: "claude-sonnet-4-6".to_string(),
        system_prompt: Some("You are helpful.".to_string()),
        max_turns: 5,
        tools: vec!["search".to_string(), "calculator".to_string()],
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.model, config.model);
    assert_eq!(deserialized.system_prompt, config.system_prompt);
    assert_eq!(deserialized.max_turns, config.max_turns);
    assert_eq!(deserialized.tools, config.tools);
}

#[test]
fn conversation_turn_serialization() {
    let turn = ConversationTurn {
        role: "user".to_string(),
        content: "Hello, world!".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
    };
    let json = serde_json::to_string(&turn).unwrap();
    let parsed: ConversationTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.role, "user");
    assert_eq!(parsed.content, "Hello, world!");
}

#[test]
fn agent_event_variants() {
    let events = vec![
        AgentEvent::Message("hello".to_string()),
        AgentEvent::ToolCall(vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
        }]),
        AgentEvent::ToolResult(ToolCallResult {
            tool_name: "search".to_string(),
            result: "found".to_string(),
            is_error: false,
        }),
        AgentEvent::Error("something failed".to_string()),
        AgentEvent::Done("final answer".to_string()),
    ];

    // Verify serialization roundtrip for all variants
    for event in &events {
        let json = serde_json::to_string(event).unwrap();
        let parsed: AgentEvent = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&parsed).unwrap();
        assert_eq!(json, json2);
    }

    // Verify variant count
    assert_eq!(events.len(), 5);
}

#[test]
fn conversation_turn_with_tool_calls() {
    let turn = ConversationTurn {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![
            ToolCallInfo {
                id: "tc_1".to_string(),
                name: "file_read".to_string(),
                arguments: r#"{"path":"/tmp/test"}"#.to_string(),
            },
        ],
        tool_call_id: None,
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
    };
    let json = serde_json::to_string(&turn).unwrap();
    let parsed: ConversationTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.tool_calls.len(), 1);
    assert_eq!(parsed.tool_calls[0].name, "file_read");
}

#[test]
fn tool_call_result_error() {
    let result = ToolCallResult {
        tool_name: "file_read".to_string(),
        result: "file not found".to_string(),
        is_error: true,
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: ToolCallResult = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_error);
    assert_eq!(parsed.result, "file not found");
}

#[test]
fn agent_config_with_empty_tools() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 1,
        tools: vec![],
    };
    assert!(config.tools.is_empty());
    let json = serde_json::to_string(&config).unwrap();
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert!(back.tools.is_empty());
}

#[test]
fn conversation_turn_tool_call_id() {
    let turn = ConversationTurn {
        role: "tool".to_string(),
        content: "result data".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: Some("tc_123".to_string()),
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
    };
    let json = serde_json::to_string(&turn).unwrap();
    let parsed: ConversationTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.tool_call_id, Some("tc_123".to_string()));
}

// --- Additional types tests ---

#[test]
fn tool_call_info_equality() {
    let tc1 = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: r#"{"q":"test"}"#.to_string(),
    };
    let tc2 = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: r#"{"q":"test"}"#.to_string(),
    };
    assert_eq!(tc1, tc2);
}

#[test]
fn tool_call_info_inequality() {
    let tc1 = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: "{}".to_string(),
    };
    let tc2 = ToolCallInfo {
        id: "tc_2".to_string(),
        name: "search".to_string(),
        arguments: "{}".to_string(),
    };
    assert_ne!(tc1, tc2);
}

#[test]
fn tool_definition_serialization() {
    let def = ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDef {
            name: "calculator".to_string(),
            description: "Performs calculations".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "expr": {"type": "string"}
                }
            }),
        },
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.function.name, "calculator");
    assert_eq!(parsed.function.description, "Performs calculations");
}

#[test]
fn tool_definition_default() {
    let def = ToolDefinition::default();
    assert_eq!(def.tool_type, "function");
    assert!(def.function.name.is_empty());
    assert!(def.function.description.is_empty());
    // Default parameters is a valid JSON schema object, not null
    assert!(def.function.parameters.is_object());
}

#[test]
fn agent_state_variants() {
    assert_ne!(AgentState::Idle, AgentState::Thinking);
    assert_ne!(AgentState::Thinking, AgentState::ExecutingTool);
    assert_ne!(AgentState::ExecutingTool, AgentState::Responding);
    assert_ne!(AgentState::Responding, AgentState::Idle);
}

#[test]
fn agent_state_serialization() {
    for state in &[AgentState::Idle, AgentState::Thinking, AgentState::ExecutingTool, AgentState::Responding] {
        let json = serde_json::to_string(&state).unwrap();
        let parsed: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(*state, parsed);
    }
}

#[test]
fn chat_options_default() {
    let opts = ChatOptions::default();
    assert_eq!(opts.max_tokens, Some(8192));
    assert_eq!(opts.temperature, Some(0.7));
}

#[test]
fn chat_options_serialization() {
    let opts = ChatOptions {
        max_tokens: Some(4096),
        temperature: Some(0.5),
        top_p: None,
        stop: Some(vec!["\n".to_string()]),
    };
    let json = serde_json::to_string(&opts).unwrap();
    let parsed: ChatOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.max_tokens, Some(4096));
    assert_eq!(parsed.temperature, Some(0.5));
    assert_eq!(parsed.stop, Some(vec!["\n".to_string()]));
}

#[test]
fn tool_call_result_success() {
    let result = ToolCallResult {
        tool_name: "search".to_string(),
        result: "found it".to_string(),
        is_error: false,
    };
    assert!(!result.is_error);
    let json = serde_json::to_string(&result).unwrap();
    let parsed: ToolCallResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.tool_name, "search");
}

#[test]
fn conversation_turn_clone() {
    let turn = ConversationTurn {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
    };
    let cloned = turn.clone();
    assert_eq!(cloned.role, "user");
    assert_eq!(cloned.content, "Hello");
}

#[test]
fn tool_call_info_clone() {
    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "test".to_string(),
        arguments: "{}".to_string(),
    };
    let cloned = tc.clone();
    assert_eq!(cloned.id, "tc_1");
    assert_eq!(cloned.name, "test");
}

#[test]
fn agent_event_done_matches() {
    let event = AgentEvent::Done("result".to_string());
    assert!(matches!(event, AgentEvent::Done(_)));

    let event = AgentEvent::Error("err".to_string());
    assert!(matches!(event, AgentEvent::Error(_)));

    let event = AgentEvent::Message("msg".to_string());
    assert!(matches!(event, AgentEvent::Message(_)));
}

#[test]
fn tool_definition_custom() {
    let def = ToolDefinition {
        tool_type: "custom".to_string(),
        function: ToolFunctionDef {
            name: "my_tool".to_string(),
            description: "Custom tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    };
    assert_eq!(def.tool_type, "custom");
    assert_eq!(def.function.name, "my_tool");
}

// --- repair_tool_message_pairs tests ---

fn make_turn(role: &str, content: &str) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
    }
}

fn make_assistant_with_tc(content: &str, ids: &[&str]) -> ConversationTurn {
    ConversationTurn {
        role: "assistant".to_string(),
        content: content.to_string(),
        tool_calls: ids.iter().map(|id| ToolCallInfo {
            id: id.to_string(),
            name: "tool".to_string(),
            arguments: "{}".to_string(),
        }).collect(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
    }
}

fn make_tool_response(content: &str, tc_id: &str) -> ConversationTurn {
    ConversationTurn {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: Some(tc_id.to_string()),
        timestamp: String::new(),
        reasoning_content: None,
    }
}

#[test]
fn repair_tool_pairs_normal_pair_untouched() {
    let mut msgs = vec![
        make_turn("system", "sys"),
        make_turn("user", "hello"),
        make_assistant_with_tc("", &["call_A"]),
        make_tool_response("result", "call_A"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[2].tool_calls.len(), 1);
    assert_eq!(msgs[3].tool_call_id, Some("call_A".to_string()));
}

#[test]
fn repair_tool_pairs_orphan_at_start_removed() {
    let mut msgs = vec![
        make_turn("system", "sys"),
        make_tool_response("orphan", "unknown_id"),
        make_turn("user", "hello"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
}

#[test]
fn repair_tool_pairs_multiple_orphans_at_start_removed() {
    let mut msgs = vec![
        make_tool_response("a", "id_a"),
        make_tool_response("b", "id_b"),
        make_turn("user", "hello"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
}

#[test]
fn repair_tool_pairs_mismatched_id_removed() {
    let mut msgs = vec![
        make_turn("user", "hello"),
        make_assistant_with_tc("", &["call_A"]),
        make_turn("user", "next"),
        make_tool_response("result", "call_B"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[3 - 1].role, "user"); // last is user, tool removed
}

#[test]
fn repair_tool_pairs_missing_tool_response_clears_calls() {
    let mut msgs = vec![
        make_assistant_with_tc("", &["call_A", "call_B"]),
        make_tool_response("result_a", "call_A"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].tool_calls.len(), 1);
    assert_eq!(msgs[0].tool_calls[0].id, "call_A");
}

#[test]
fn repair_tool_pairs_partial_response_keeps_matched() {
    let mut msgs = vec![
        make_assistant_with_tc("", &["call_A", "call_B", "call_C"]),
        make_tool_response("a", "call_A"),
        make_tool_response("c", "call_C"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].tool_calls.len(), 2);
    let ids: Vec<&str> = msgs[0].tool_calls.iter().map(|tc| tc.id.as_str()).collect();
    assert!(ids.contains(&"call_A"));
    assert!(ids.contains(&"call_C"));
}

#[test]
fn repair_tool_pairs_empty_history_ok() {
    let mut msgs: Vec<ConversationTurn> = Vec::new();
    repair_tool_message_pairs(&mut msgs);
    assert!(msgs.is_empty());
}

#[test]
fn repair_tool_pairs_pure_user_conversation_ok() {
    let mut msgs = vec![
        make_turn("user", "hi"),
        make_turn("assistant", "hello"),
        make_turn("user", "bye"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 3);
}

#[test]
fn repair_tool_pairs_system_message_preserved() {
    let mut msgs = vec![
        make_turn("system", "you are helpful"),
        make_tool_response("orphan", "x"),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "system");
}

// --- duplicate tool_call_id dedup tests (Pass 0) ---
// Reproduces the cluster_rpc continuation bug: async placeholder + injected result
// both carry the same tool_call_id and must collapse to one before sending to the LLM.

#[test]
fn repair_tool_pairs_dedupes_duplicate_tool_call_id_keeps_last() {
    let mut msgs = vec![
        make_turn("user", "check"),
        make_assistant_with_tc("", &["call_X"]),
        make_tool_response("placeholder __ASYNC__", "call_X"),
        make_tool_response("real callback result", "call_X"),
    ];
    repair_tool_message_pairs(&mut msgs);
    let tool_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tool_msgs.len(), 1, "duplicate must collapse to one");
    assert_eq!(tool_msgs[0].content, "real callback result", "must keep the last");
}

#[test]
fn repair_tool_pairs_dedupes_across_other_messages() {
    let mut msgs = vec![
        make_turn("user", "go"),
        make_assistant_with_tc("", &["call_X"]),
        make_tool_response("placeholder", "call_X"),
        make_turn("assistant", "thinking..."),
        make_tool_response("real result", "call_X"),
    ];
    repair_tool_message_pairs(&mut msgs);
    let tool_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tool_msgs.len(), 1);
    assert_eq!(tool_msgs[0].content, "real result");
}

#[test]
fn repair_tool_pairs_keeps_distinct_tool_call_ids() {
    let mut msgs = vec![
        make_turn("user", "go"),
        make_assistant_with_tc("", &["call_A", "call_B"]),
        make_tool_response("res A", "call_A"),
        make_tool_response("res B", "call_B"),
    ];
    repair_tool_message_pairs(&mut msgs);
    let tool_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tool_msgs.len(), 2, "distinct ids must both survive");
}

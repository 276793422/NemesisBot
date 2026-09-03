use super::*;

#[test]
fn agent_config_default() {
    let config = AgentConfig::default();
    assert_eq!(config.model, "gpt-4");
    assert!(config.system_prompt.is_none());
    assert_eq!(config.max_turns, 100);
    assert!(config.tools.is_empty());
}

#[test]
fn agent_config_serialization_roundtrip() {
    let config = AgentConfig {
        model: "claude-sonnet-4-6".to_string(),
        system_prompt: Some("You are helpful.".to_string()),
        max_turns: 5,
        tools: vec!["search".to_string(), "calculator".to_string()],
        models: std::collections::HashMap::new(),
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
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
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
        tool_calls: vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "file_read".to_string(),
            arguments: r#"{"path":"/tmp/test"}"#.to_string(),
        }],
        tool_call_id: None,
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
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
        models: std::collections::HashMap::new(),
    };
    assert!(config.tools.is_empty());
    let json = serde_json::to_string(&config).unwrap();
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert!(back.tools.is_empty());
}

#[test]
fn agent_config_models_serde_roundtrip() {
    let mut models = std::collections::HashMap::new();
    models.insert("flash".to_string(), "deepseek-v4-flash".to_string());
    models.insert("pro".to_string(), "deepseek-v4-pro".to_string());
    let config = AgentConfig {
        model: "deepseek-v4-flash".to_string(),
        system_prompt: None,
        max_turns: 10,
        tools: vec![],
        models,
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.models.len(), 2);
    assert_eq!(back.models.get("flash").unwrap(), "deepseek-v4-flash");
    assert_eq!(back.models.get("pro").unwrap(), "deepseek-v4-pro");
}

#[test]
fn agent_config_models_default_empty_when_missing_in_json() {
    // A JSON config without "models" key → models should default to empty.
    let json = r#"{"model":"gpt-4","system_prompt":null,"max_turns":5,"tools":[]}"#;
    let config: AgentConfig = serde_json::from_str(json).unwrap();
    assert!(config.models.is_empty());
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
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
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
    for state in &[
        AgentState::Idle,
        AgentState::Thinking,
        AgentState::ExecutingTool,
        AgentState::Responding,
    ] {
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
        reasoning_effort: None,
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
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
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
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

fn make_assistant_with_tc(content: &str, ids: &[&str]) -> ConversationTurn {
    ConversationTurn {
        role: "assistant".to_string(),
        content: content.to_string(),
        tool_calls: ids
            .iter()
            .map(|id| ToolCallInfo {
                id: id.to_string(),
                name: "tool".to_string(),
                arguments: "{}".to_string(),
            })
            .collect(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
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
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
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
    // G2 (U2): the orphaned call_A now gets a SYNTHETIC unknown-outcome
    // result instead of being dropped; the mismatched call_B result is still
    // removed (Pass 1). The assistant keeps its call.
    assert_eq!(msgs.len(), 4);
    let assistant = &msgs[1];
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].id, "call_A");
    let synth = &msgs[2];
    assert_eq!(synth.role, "tool");
    assert_eq!(synth.tool_call_id.as_deref(), Some("call_A"));
    assert!(synth.content.contains(TOOL_OUTCOME_UNKNOWN));
    let no_b = msgs
        .iter()
        .any(|m| m.tool_call_id.as_deref() == Some("call_B"));
    assert!(!no_b, "mismatched call_B result removed");
}

#[test]
fn repair_tool_pairs_missing_tool_response_clears_calls() {
    let mut msgs = vec![
        make_assistant_with_tc("", &["call_A", "call_B"]),
        make_tool_response("result_a", "call_A"),
    ];
    repair_tool_message_pairs(&mut msgs);
    // G2 (U2): call_B gets a synthetic result; the assistant keeps BOTH
    // calls (previously call_B was dropped from tool_calls). The synthetic
    // inserts at assistant+1 (call order), the real result_A follows it.
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].tool_calls.len(), 2);
    let synth = &msgs[1];
    assert_eq!(synth.role, "tool");
    assert_eq!(synth.tool_call_id.as_deref(), Some("call_B"));
    assert!(synth.content.contains(TOOL_OUTCOME_UNKNOWN));
    assert_eq!(msgs[2].tool_call_id.as_deref(), Some("call_A"));
    assert_eq!(msgs[2].content, "result_a");
}

#[test]
fn repair_tool_pairs_partial_response_keeps_matched() {
    let mut msgs = vec![
        make_assistant_with_tc("", &["call_A", "call_B", "call_C"]),
        make_tool_response("a", "call_A"),
        make_tool_response("c", "call_C"),
    ];
    repair_tool_message_pairs(&mut msgs);
    // G2 (U2): all three calls survive on the assistant; call_B gains a
    // synthetic result inserted after the assistant (matching the model's
    // call order, before the later real results).
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].tool_calls.len(), 3);
    let synth = &msgs[1];
    assert_eq!(synth.role, "tool");
    assert_eq!(synth.tool_call_id.as_deref(), Some("call_B"));
    assert!(synth.content.contains(TOOL_OUTCOME_UNKNOWN));
    assert_eq!(msgs[2].content, "a");
    assert_eq!(msgs[3].content, "c");
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
    assert_eq!(
        tool_msgs[0].content, "real callback result",
        "must keep the last"
    );
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

// ---------------------------------------------------------------------------
// G2 (U2): repair_tool_message_pairs synthesizes TOOL_OUTCOME_UNKNOWN results
// ---------------------------------------------------------------------------

/// Helper: build a ConversationTurn quickly.
fn turn(
    role: &str,
    content: &str,
    tool_calls: Vec<ToolCallInfo>,
    tool_call_id: Option<String>,
) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls,
        tool_call_id,
        timestamp: "2026-08-21T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

fn call(id: &str, name: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

/// An orphaned tool_call gets a synthetic tool result inserted right after its
/// assistant turn (paired, model-visible, carries TOOL_OUTCOME_UNKNOWN), and
/// the tool_call is NOT dropped from the assistant turn.
#[test]
fn test_repair_synthesizes_unknown_outcome_tool_result() {
    let mut msgs = vec![
        turn("system", "sys", vec![], None),
        turn("user", "hi", vec![], None),
        turn(
            "assistant",
            "let me check",
            vec![call("call_1", "read_file")],
            None,
        ),
        // NO tool result for call_1 (the orphan case: crash / truncation).
        turn("user", "and then?", vec![], None),
    ];
    let snapshot_len = msgs.len();
    repair_tool_message_pairs(&mut msgs);

    // The assistant turn still carries its tool_call.
    let assistant = msgs.iter().find(|m| m.role == "assistant").unwrap();
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].id, "call_1");

    // A synthetic tool result exists, positioned after the assistant turn,
    // with the call id pointing back and the unknown-outcome wording.
    let assistant_pos = msgs.iter().position(|m| m.role == "assistant").unwrap();
    let synth = &msgs[assistant_pos + 1];
    assert_eq!(synth.role, "tool");
    assert_eq!(synth.tool_call_id.as_deref(), Some("call_1"));
    assert!(synth.content.contains(TOOL_OUTCOME_UNKNOWN));
    assert!(synth.content.contains("只读或幂等"));
    assert!(synth.content.contains("验证外部状态"));

    // Exactly one turn was added.
    assert_eq!(msgs.len(), snapshot_len + 1);
}

/// Multiple orphaned calls in one assistant turn get synthetic results in the
/// model's original call order.
#[test]
fn test_repair_synthesizes_multiple_unknown_outcomes_in_call_order() {
    let mut msgs = vec![
        turn("user", "do two things", vec![], None),
        turn(
            "assistant",
            "two calls",
            vec![call("call_a", "grep"), call("call_b", "exec")],
            None,
        ),
        // Only call_b has a result, and it comes AFTER where both synthetics
        // would insert — call_a is the orphan.
        turn("tool", "ok", vec![], Some("call_b".to_string())),
    ];
    repair_tool_message_pairs(&mut msgs);

    let assistant_pos = msgs.iter().position(|m| m.role == "assistant").unwrap();
    // Next turn after the assistant must be the synthetic result for call_a
    // (the FIRST call — insertion order matches the model's call order).
    let synth_a = &msgs[assistant_pos + 1];
    assert_eq!(synth_a.role, "tool");
    assert_eq!(synth_a.tool_call_id.as_deref(), Some("call_a"));
    // Then the real result for call_b.
    let real_b = &msgs[assistant_pos + 2];
    assert_eq!(real_b.tool_call_id.as_deref(), Some("call_b"));
    assert_eq!(real_b.content, "ok");
    // No additional synthetic for call_b.
    assert_eq!(msgs.len(), 4);
}

/// repair only transforms the projection copy: the caller's Vec is the copy
/// build_messages made; this test pins that the function's contract is
/// "in-place on the given Vec" — the non-mutation of instance history is a
/// build_messages property, verified separately in loop tests. Here we pin
/// that a fully-paired history is a no-op (byte-identical semantics).
#[test]
fn test_repair_paired_history_is_noop() {
    let mut msgs = vec![
        turn("system", "sys", vec![], None),
        turn("user", "hi", vec![], None),
        turn(
            "assistant",
            "checking",
            vec![call("call_1", "read_file")],
            None,
        ),
        turn("tool", "file contents", vec![], Some("call_1".to_string())),
    ];
    let before = msgs.clone();
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs, before);
}

/// Orphaned tool RESULTS (result without a preceding call) are still removed
/// (Pass 1 unchanged) — the synthesis only addresses the missing-result side.
#[test]
fn test_repair_still_removes_orphaned_tool_results() {
    let mut msgs = vec![
        turn("user", "hi", vec![], None),
        turn(
            "tool",
            "ghost result",
            vec![],
            Some("call_nonexistent".to_string()),
        ),
    ];
    repair_tool_message_pairs(&mut msgs);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
}

// ---------------------------------------------------------------------------
// X1 (U3 projection prune): per-turn model-facing projection unit tests.
// ---------------------------------------------------------------------------

fn x1_turn(role: &str, content: &str) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

/// Non-tool turns NEVER fold - even oversized user/assistant content reaches
/// the provider verbatim (pruning is a tool-result policy only).
#[test]
fn test_model_facing_non_tool_passthrough() {
    let big: String = "u".repeat(20_000);
    for role in ["user", "assistant", "system"] {
        let t = x1_turn(role, &big);
        assert_eq!(
            t.model_facing_content(),
            big,
            "non-tool turns must never fold"
        );
    }
}

/// Oversized tool result without a recorded override folds to
/// head + marker + tail: bounded, marker carries the tool name, head/tail
/// bytes match the original's first/last 3600 chars.
#[test]
fn test_model_facing_tool_pruned_shape() {
    let original: String = {
        let head: String = "H".repeat(3_600);
        let mid: String = "M".repeat(5_000);
        let tail: String = "T".repeat(3_600);
        format!("{head}{mid}{tail}")
    };
    let mut t = x1_turn("tool", &original);
    t.tool_name = Some("grep".to_string());
    let projected = t.model_facing_content().into_owned();
    assert!(
        projected.chars().count() < original.chars().count(),
        "oversized tool result must shrink"
    );
    assert!(
        projected.contains("grep"),
        "marker must carry the tool name"
    );
    assert!(projected.starts_with(&"H".repeat(3_600)));
    assert!(projected.ends_with(&"T".repeat(3_600)));
}

/// A recorded override (spill locator / guard nudges) wins verbatim - even
/// over content that would otherwise pass through unpruned.
#[test]
fn test_model_facing_override_wins() {
    let mut t = x1_turn("tool", "small");
    t.tool_result_projection = Some("SPILLED: see file".to_string());
    assert_eq!(t.model_facing_content(), "SPILLED: see file");
}

/// In-budget tool results pass through untouched.
#[test]
fn test_model_facing_small_tool_passthrough() {
    let t = x1_turn("tool", "ok");
    assert_eq!(t.model_facing_content(), "ok");
}

/// FOLD IDEMPOTENCE: the prune output is itself under the inline threshold,
/// so projecting an already-projected turn (old sessions whose stored tool
/// content was pruned by the pre-X1 write-time gate) is a byte-level no-op.
#[test]
fn test_model_facing_projection_idempotent() {
    let original: String = "x".repeat(20_000);
    let mut t = x1_turn("tool", &original);
    t.tool_name = Some("exec".to_string());
    let once = t.model_facing_content().into_owned();
    let mut t2 = x1_turn("tool", &once);
    t2.tool_name = Some("exec".to_string());
    assert_eq!(
        t2.model_facing_content().as_ref(),
        once,
        "second projection must be a no-op (old-session compatibility)"
    );
}

/// Missing tool_name (old sessions) falls back to the generic "tool" in the
/// marker instead of failing.
#[test]
fn test_model_facing_tool_name_fallback() {
    let original: String = "y".repeat(20_000);
    let t = x1_turn("tool", &original);
    let projected = t.model_facing_content().into_owned();
    assert!(projected.contains("tool"), "marker falls back to tool");
}

/// ToolDefinition::default() has the "function" tool_type and empty fn def
/// (second file-level check near other trailing defaults; kept separate from
/// the earlier `tool_definition_default` to avoid a name collision).
#[test]
fn tool_definition_default_recheck() {
    let d = ToolDefinition::default();
    assert_eq!(d.tool_type, "function");
    assert_eq!(d.function.name, "");
}

fn assistant_with_calls(ids: &[&str]) -> ConversationTurn {
    ConversationTurn {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: ids
            .iter()
            .map(|id| ToolCallInfo {
                id: id.to_string(),
                name: "t".to_string(),
                arguments: "{}".to_string(),
            })
            .collect(),
        tool_call_id: None,
        timestamp: "2026-08-25T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

fn tool_result_turn(id: Option<&str>, content: &str) -> ConversationTurn {
    ConversationTurn {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: id.map(|s| s.to_string()),
        timestamp: "2026-08-25T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

/// Two tool results with the SAME id: pass 0 scans in REVERSE keeping the
/// LAST occurrence (async placeholder followed by the real callback result —
/// the real one wins), so the earlier duplicate is removed.
#[test]
fn repair_removes_duplicate_tool_ids() {
    let mut msgs = vec![
        assistant_with_calls(&["a"]),
        tool_result_turn(Some("a"), "r1"),
        tool_result_turn(Some("a"), "r2"),
    ];
    repair_tool_message_pairs(&mut msgs);
    let tools: Vec<_> = msgs.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 1, "duplicate tool result dropped");
    assert_eq!(tools[0].content, "r2", "the LAST occurrence is kept");
}

/// A tool result WITHOUT an id is an orphan → removed.
#[test]
fn repair_removes_tool_result_without_id() {
    let mut msgs = vec![
        assistant_with_calls(&["a"]),
        tool_result_turn(None, "orphan"),
        tool_result_turn(Some("a"), "ok"),
    ];
    repair_tool_message_pairs(&mut msgs);
    let tools: Vec<_> = msgs.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].content, "ok");
}

/// Assistant issued calls a+b but only a has a result → a synthesized
/// unknown-outcome tool result is INSERTED for the unanswered call b (pass-2
/// insertion, keeps pairs complete for providers).
#[test]
fn repair_synthesizes_result_for_unanswered_call() {
    let mut msgs = vec![
        assistant_with_calls(&["a", "b"]),
        tool_result_turn(Some("a"), "ra"),
    ];
    repair_tool_message_pairs(&mut msgs);
    // Both original calls stay on the assistant turn.
    assert_eq!(msgs[0].tool_calls.len(), 2);
    // A synthesized tool result for 'b' was inserted right after it.
    let b_result = msgs
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("b"))
        .expect("synthesized result for unanswered call 'b'");
    assert!(
        b_result.content.contains("TOOL_OUTCOME_UNKNOWN"),
        "marker: {}",
        b_result.content
    );
}

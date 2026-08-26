use super::*;

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["search".to_string()],
        models: std::collections::HashMap::new(),
    }
}

#[test]
fn new_instance_has_system_prompt() {
    let instance = AgentInstance::new(test_config());
    let history = instance.get_history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[0].content, "You are a test assistant.");
    assert_eq!(instance.state(), AgentState::Idle);
}

#[test]
fn add_messages_and_get_history() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    instance.add_assistant_message("Hi there!", Vec::new(), None);

    let history = instance.get_history();
    // system + user + assistant = 3
    assert_eq!(history.len(), 3);
    assert_eq!(history[1].role, "user");
    assert_eq!(history[1].content, "Hello");
    assert_eq!(history[2].role, "assistant");
    assert_eq!(history[2].content, "Hi there!");
}

#[test]
fn add_tool_result() {
    let instance = AgentInstance::new(test_config());
    let tool_calls = vec![ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: r#"{"query":"rust"}"#.to_string(),
    }];
    instance.add_assistant_message("", tool_calls, None);
    instance.add_tool_result("tc_1", "Results for rust");

    let history = instance.get_history();
    assert_eq!(history.len(), 3); // system + assistant + tool
    let tool_turn = &history[2];
    assert_eq!(tool_turn.role, "tool");
    assert_eq!(tool_turn.tool_call_id.as_deref(), Some("tc_1"));
    assert_eq!(tool_turn.content, "Results for rust");
}

#[test]
fn state_transitions() {
    let instance = AgentInstance::new(test_config());
    assert_eq!(instance.state(), AgentState::Idle);

    // Idle -> Thinking
    assert!(instance.start_thinking());
    assert_eq!(instance.state(), AgentState::Thinking);

    // Cannot transition from Thinking to Thinking again
    assert!(!instance.start_thinking());

    // Thinking -> ExecutingTool
    assert!(instance.start_tool_execution());
    assert_eq!(instance.state(), AgentState::ExecutingTool);

    // ExecutingTool -> Responding
    assert!(instance.start_responding());
    assert_eq!(instance.state(), AgentState::Responding);

    // Responding -> Idle
    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);
}

#[test]
fn clear_history_preserves_system_prompt() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    instance.add_assistant_message("Hi!", Vec::new(), None);
    assert_eq!(instance.get_history().len(), 3);

    instance.clear_history();
    let history = instance.get_history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "system");
}

#[test]
fn compress_history_keeps_system_and_half_turns() {
    let instance = AgentInstance::new(test_config());
    // Add 6 turns: u1, a1, u2, a2, u3, a3
    instance.add_user_message("u1");
    instance.add_assistant_message("a1", Vec::new(), None);
    instance.add_user_message("u2");
    instance.add_assistant_message("a2", Vec::new(), None);
    instance.add_user_message("u3");
    instance.add_assistant_message("a3", Vec::new(), None);
    // system + 6 turns = 7
    assert_eq!(instance.get_history().len(), 7);

    instance.compress_history();
    let history = instance.get_history();

    // system prompt + compression note + last 3 of 6 turns = 5
    // keep_count = 6/2 = 3, start = 6-3 = 3, skip(3) yields 3 turns: a2, u3, a3
    assert_eq!(history.len(), 5);
    // First message is system prompt
    assert_eq!(history[0].role, "system");
    assert!(history[0].content.contains("test assistant"));
    // Second message is compression note
    assert_eq!(history[1].role, "system");
    assert!(history[1].content.contains("[Session compressed at"));
    // skip(3) removes u1, a1, u2, keeps a2, u3, a3
    assert!(history[2].content.contains("a2"));
}

#[test]
fn compress_history_noop_on_short_history() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    assert_eq!(instance.get_history().len(), 2);

    instance.compress_history();
    let history = instance.get_history();
    // Should remain unchanged (too short to compress)
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].content, "Hello");
}

// --- Additional instance tests ---

#[test]
fn new_instance_without_system_prompt() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    let instance = AgentInstance::new(config);
    assert!(instance.get_history().is_empty());
    assert_eq!(instance.message_count(), 0);
}

#[test]
fn instance_unique_ids() {
    let a = AgentInstance::new(test_config());
    let b = AgentInstance::new(test_config());
    assert_ne!(a.id(), b.id());
    assert!(a.id() > 0);
    assert!(b.id() > 0);
}

#[test]
fn instance_config_access() {
    let instance = AgentInstance::new(test_config());
    assert_eq!(instance.config().model, "test-model");
    assert_eq!(instance.config().max_turns, 5);
}

#[test]
fn instance_state_transitions_invalid() {
    let instance = AgentInstance::new(test_config());

    // Cannot go to ExecutingTool from Idle
    assert!(!instance.start_tool_execution());

    // Cannot go to Responding from Idle
    assert!(!instance.start_responding());

    // Can go to Idle from any state via finish
    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);
}

#[test]
fn instance_state_thinking_to_responding() {
    let instance = AgentInstance::new(test_config());
    instance.start_thinking();
    // Can go directly from Thinking to Responding
    assert!(instance.start_responding());
    assert_eq!(instance.state(), AgentState::Responding);
}

#[test]
fn instance_add_messages_increments_count() {
    let instance = AgentInstance::new(test_config());

    assert_eq!(instance.message_count(), 0);
    instance.add_user_message("Hello");
    assert_eq!(instance.message_count(), 1);
    instance.add_assistant_message("Hi", Vec::new(), None);
    assert_eq!(instance.message_count(), 2);
    instance.add_tool_result("tc_1", "Result");
    assert_eq!(instance.message_count(), 3);
}

#[test]
fn instance_message_count_excludes_system() {
    let instance = AgentInstance::new(test_config());
    // System prompt is added automatically
    assert_eq!(instance.get_history().len(), 1);
    assert_eq!(instance.message_count(), 0); // system excluded
}

#[test]
fn instance_summary_cache_default_none() {
    // A fresh instance has no summary cache.
    let instance = AgentInstance::new(test_config());
    assert!(instance.get_summary_cache().is_none());
}

#[test]
fn instance_summary_cache_set_get_roundtrip() {
    let instance = AgentInstance::new(test_config());

    instance.set_summary_cache(Some(SummaryCache {
        covers_up_to: 4,
        text: "Earlier we discussed X and Y.".to_string(),
    }));

    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(cache.covers_up_to, 4);
    assert_eq!(cache.text, "Earlier we discussed X and Y.");
}

#[test]
fn instance_summary_cache_clear() {
    let instance = AgentInstance::new(test_config());
    instance.set_summary_cache(Some(SummaryCache {
        covers_up_to: 3,
        text: "stub".to_string(),
    }));
    assert!(instance.get_summary_cache().is_some());

    instance.set_summary_cache(None);
    assert!(instance.get_summary_cache().is_none());
}

#[test]
fn instance_context_window() {
    let mut instance = AgentInstance::new(test_config());
    assert_eq!(instance.context_window(), 32000);

    instance.set_context_window(64000);
    assert_eq!(instance.context_window(), 64000);
}

#[test]
fn instance_metadata() {
    let instance = AgentInstance::new(test_config());

    // Default metadata is Null
    assert!(instance.metadata().is_null());

    instance.set_metadata(serde_json::json!({"key": "value"}));
    let meta = instance.metadata();
    assert_eq!(meta["key"], "value");
}

#[test]
fn instance_workspace() {
    let mut instance = AgentInstance::new(test_config());
    assert!(instance.workspace().as_os_str().is_empty());

    instance.set_workspace(PathBuf::from("/tmp/workspace"));
    assert_eq!(instance.workspace(), &PathBuf::from("/tmp/workspace"));
}

#[test]
fn instance_max_iterations() {
    let mut instance = AgentInstance::new(test_config());
    assert_eq!(instance.max_iterations(), 60);

    instance.set_max_iterations(50);
    assert_eq!(instance.max_iterations(), 50);
}

#[test]
fn instance_subagents() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.subagents().is_empty());

    instance.set_subagents(vec!["agent_a".to_string(), "agent_b".to_string()]);
    let agents = instance.subagents();
    assert_eq!(agents.len(), 2);
    assert!(agents.contains(&"agent_a".to_string()));
    assert!(agents.contains(&"agent_b".to_string()));
}

#[test]
fn instance_skills_filter() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.skills_filter().is_empty());

    instance.set_skills_filter(vec!["skill1".to_string()]);
    let filter = instance.skills_filter();
    assert_eq!(filter.len(), 1);
    assert!(filter.contains(&"skill1".to_string()));
}

#[test]
fn instance_fallback_candidates() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.fallback_candidates().is_empty());

    instance.set_fallback_candidates(vec!["model_a".to_string(), "model_b".to_string()]);
    let candidates = instance.fallback_candidates();
    assert_eq!(candidates.len(), 2);
}

#[test]
fn instance_provider_meta() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.provider_meta().is_none());

    instance.set_provider_meta(serde_json::json!({"name": "openai"}));
    let meta = instance.provider_meta();
    assert!(meta.is_some());
    assert_eq!(meta.unwrap()["name"], "openai");
}

#[test]
fn instance_truncate_to() {
    let instance = AgentInstance::new(test_config());
    for i in 0..10 {
        instance.add_user_message(&format!("msg_{}", i));
    }
    // system + 10 user messages = 11
    assert_eq!(instance.get_history().len(), 11);

    instance.truncate_to(5);
    let history = instance.get_history();
    assert_eq!(history.len(), 5);
}

#[test]
fn instance_truncate_to_more_than_history() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("msg1");
    instance.add_user_message("msg2");
    // system + 2 = 3

    instance.truncate_to(100);
    assert_eq!(instance.get_history().len(), 3); // No change
}

#[test]
fn instance_set_history() {
    let instance = AgentInstance::new(test_config());

    let new_history = vec![
        ConversationTurn {
            role: "system".to_string(),
            content: "Custom system".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "Custom user".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];

    instance.set_history(new_history);
    let history = instance.get_history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "Custom system");
    assert_eq!(history[1].content, "Custom user");
}

#[test]
fn instance_clear_history_no_system_prompt() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    let instance = AgentInstance::new(config);
    instance.add_user_message("Hello");
    instance.add_assistant_message("Hi", Vec::new(), None);

    instance.clear_history();
    assert!(instance.get_history().is_empty());
}

#[test]
fn instance_history_is_append_only() {
    // S3.1: push_turn no longer truncates — history is strictly append-only.
    // Bounding (drop-oldest with C-aware index adjustment) is the session
    // store's responsibility, never the instance's, so covers_up_to stays valid.
    let instance = AgentInstance::new(test_config());

    for i in 0..101 {
        instance.add_user_message(&format!("msg_{}", i));
    }

    let history = instance.get_history();
    // system + 101 user turns, nothing dropped.
    assert_eq!(history.len(), 102);
    assert_eq!(history[0].role, "system");
    // Oldest message is still there (no truncation).
    assert_eq!(history[1].content, "msg_0");
    // Most recent preserved.
    let last_content = history.last().unwrap().content.clone();
    assert_eq!(last_content, "msg_100");
}

#[test]
fn instance_compress_with_many_turns() {
    let instance = AgentInstance::new(test_config());
    // Add 20 turns
    for i in 0..20 {
        instance.add_user_message(&format!("u{}", i));
        instance.add_assistant_message(&format!("a{}", i), Vec::new(), None);
    }
    // system + 40 turns = 41
    assert_eq!(instance.get_history().len(), 41);

    instance.compress_history();
    let history = instance.get_history();
    // Should be significantly smaller
    assert!(history.len() < 41);
    // System prompt preserved
    assert_eq!(history[0].role, "system");
    // Compression note present
    assert!(history[1].content.contains("[Session compressed at"));
}

#[test]
fn instance_tool_result_message() {
    let instance = AgentInstance::new(test_config());
    let tool_calls = vec![ToolCallInfo {
        id: "tc_abc".to_string(),
        name: "calculator".to_string(),
        arguments: "{}".to_string(),
    }];
    instance.add_assistant_message("", tool_calls, None);
    instance.add_tool_result("tc_abc", "42");

    let history = instance.get_history();
    let tool_msg = history.iter().find(|t| t.role == "tool").unwrap();
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("tc_abc"));
    assert_eq!(tool_msg.content, "42");
}

#[test]
fn instance_assistant_with_multiple_tool_calls() {
    let instance = AgentInstance::new(test_config());
    let tool_calls = vec![
        ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"rust"}"#.to_string(),
        },
        ToolCallInfo {
            id: "tc_2".to_string(),
            name: "calculator".to_string(),
            arguments: r#"{"expr":"2+2"}"#.to_string(),
        },
    ];
    instance.add_assistant_message("Let me help", tool_calls, None);

    let history = instance.get_history();
    let assistant_msg = history.iter().find(|t| t.role == "assistant").unwrap();
    assert_eq!(assistant_msg.tool_calls.len(), 2);
    assert_eq!(assistant_msg.tool_calls[0].name, "search");
    assert_eq!(assistant_msg.tool_calls[1].name, "calculator");
}

// --- replace_tool_result tests (cluster_rpc continuation resume path) ---

fn make_history_with_async_placeholder() -> Vec<ConversationTurn> {
    vec![
        ConversationTurn {
            role: "system".to_string(),
            content: "sys".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "check cluster".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_X".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "tool".to_string(),
            content: "Request accepted. __CLUSTER_ASYNC__{\"task_id\":\"t1\"}".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_X".to_string()),
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ]
}

#[test]
fn replace_tool_result_overwrites_async_placeholder() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    let instance = AgentInstance::new(config);
    instance.set_history(make_history_with_async_placeholder());

    instance.replace_tool_result("tc_X", "real callback result");

    let history = instance.get_history();
    assert_eq!(history.len(), 4, "should remain 4 messages, no append");
    let tool_msgs: Vec<_> = history.iter().filter(|t| t.role == "tool").collect();
    assert_eq!(tool_msgs.len(), 1, "exactly one tool message");
    assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("tc_X"));
    assert_eq!(tool_msgs[0].content, "real callback result");
}

#[test]
fn replace_tool_result_falls_back_to_push_when_no_match() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    let instance = AgentInstance::new(config);
    instance.set_history(make_history_with_async_placeholder());

    instance.replace_tool_result("tc_OTHER", "result for other id");

    let history = instance.get_history();
    assert_eq!(history.len(), 5, "should append when no match");
    let tool_msgs: Vec<_> = history.iter().filter(|t| t.role == "tool").collect();
    assert_eq!(tool_msgs.len(), 2);
    assert!(
        tool_msgs
            .iter()
            .any(|t| t.tool_call_id.as_deref() == Some("tc_X"))
    );
    assert!(
        tool_msgs
            .iter()
            .any(|t| t.tool_call_id.as_deref() == Some("tc_OTHER"))
    );
}

#[test]
fn replace_tool_result_dedupes_multiple_matches() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    let instance = AgentInstance::new(config);
    instance.set_history(make_history_with_async_placeholder());
    instance.add_tool_result("tc_X", "second result (duplicate id)");

    let history = instance.get_history();
    assert_eq!(
        history.iter().filter(|t| t.role == "tool").count(),
        2,
        "precondition: two tool messages with same id"
    );

    instance.replace_tool_result("tc_X", "final replace");

    let history = instance.get_history();
    let tool_msgs: Vec<_> = history.iter().filter(|t| t.role == "tool").collect();
    assert_eq!(tool_msgs.len(), 1, "dedup to one");
    assert_eq!(tool_msgs[0].content, "final replace");
    assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("tc_X"));
}

// ----- W3a branch coverage -----

/// compress_history with >2 turns but NO non-system turns: early return at the
/// `non_system.is_empty()` guard — history must be left untouched (no
/// compression note injected).
#[test]
fn compress_history_all_system_turns_is_noop() {
    let instance = AgentInstance::new(test_config());
    let mk_sys = |content: &str| ConversationTurn {
        role: "system".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "t".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };
    instance.set_history(vec![mk_sys("s1"), mk_sys("s2"), mk_sys("s3")]);
    instance.compress_history();
    let history = instance.get_history();
    assert_eq!(history.len(), 3, "all-system history must not be compressed");
    assert!(
        !history.iter().any(|t| t.content.contains("Session compressed")),
        "no compression note for empty non-system set"
    );
}

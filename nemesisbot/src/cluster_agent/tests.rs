use super::*;
use nemesis_agent::instance::AgentInstance;
use nemesis_agent::r#loop::{AgentLoop, LlmProvider, LlmMessage, LlmResponse};
use nemesis_agent::session::SessionStore;
use nemesis_agent::types::{AgentConfig, AgentEvent, ConversationTurn, ToolCallInfo};
use nemesis_cluster::cluster_task::{ClusterTask, TaskSource, TaskStatus};

// -- is_async_done -------------------------------------------------------
//
// Detection now keys off the `__CLUSTER_ASYNC__` marker in tool messages
// rather than the user-facing wording, so the message template can change
// freely without breaking multi-hop chain call detection.

#[test]
fn test_is_async_done_true() {
    let convo = vec![
        make_turn("user", "请帮我联系 Alex", vec![]),
        make_turn(
            "assistant",
            "",
            vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }],
        ),
        make_turn(
            "tool",
            "Request accepted by node-X. Task ID: auto-abc | __CLUSTER_ASYNC__{\"task_id\":\"auto-abc\",\"target\":\"node-X\"}",
            vec![],
        ),
    ];
    assert!(is_async_done(&convo));
}

#[test]
fn test_is_async_done_false_normal_done() {
    let convo = vec![
        make_turn("user", "你好", vec![]),
        make_turn("assistant", "你好呀", vec![]),
        make_turn("tool", "some regular tool output with no marker", vec![]),
    ];
    assert!(!is_async_done(&convo));
}

#[test]
fn test_is_async_done_empty() {
    let convo: Vec<ConversationTurn> = vec![];
    assert!(!is_async_done(&convo));
}

// -- extract_async_info --------------------------------------------------

fn make_turn(role: &str, content: &str, tool_calls: Vec<ToolCallInfo>) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls,
        tool_call_id: None,
        timestamp: "2026-06-04T00:00:00Z".to_string(),
        reasoning_content: None,
    }
}

#[test]
fn test_extract_async_info_json_marker() {
    let tool_call = ToolCallInfo {
        id: "tc_456".to_string(),
        name: "cluster_rpc".to_string(),
        arguments: "{}".to_string(),
    };
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn("assistant", "calling tool", vec![tool_call]),
        make_turn(
            "tool",
            "__CLUSTER_ASYNC__{\"task_id\":\"child-123\"}",
            vec![],
        ),
    ];
    let result = extract_async_info(&conversation);
    assert_eq!(result, Some(("child-123".to_string(), "tc_456".to_string())));
}

#[test]
fn test_extract_async_info_text_fallback() {
    let tool_call = ToolCallInfo {
        id: "tc_789".to_string(),
        name: "cluster_rpc".to_string(),
        arguments: "{}".to_string(),
    };
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn("assistant", "calling tool", vec![tool_call]),
        make_turn("tool", "Request accepted. Task ID: child-xyz", vec![]),
    ];
    let result = extract_async_info(&conversation);
    assert_eq!(
        result,
        Some(("child-xyz".to_string(), "tc_789".to_string()))
    );
}

#[test]
fn test_extract_async_info_none() {
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn("assistant", "no tools called", vec![]),
    ];
    assert!(extract_async_info(&conversation).is_none());
}

#[test]
fn test_extract_async_info_no_tool_call_id() {
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn(
            "tool",
            "__CLUSTER_ASYNC__{\"task_id\":\"child-456\"}",
            vec![],
        ),
    ];
    assert!(extract_async_info(&conversation).is_none());
}

// -- extract_final_message -----------------------------------------------

#[test]
fn test_extract_final_message() {
    let events = vec![
        AgentEvent::Message("intermediate".to_string()),
        AgentEvent::ToolCall(vec![]),
        AgentEvent::Message("more work".to_string()),
        AgentEvent::Done("final answer".to_string()),
    ];
    assert_eq!(extract_final_message(&events), "final answer");
}

#[test]
fn test_extract_final_message_no_done() {
    let events = vec![
        AgentEvent::Message("thinking".to_string()),
        AgentEvent::Error("something broke".to_string()),
    ];
    assert_eq!(extract_final_message(&events), "");
}

#[test]
fn test_extract_final_message_returns_last_done() {
    let events = vec![
        AgentEvent::Done("first done".to_string()),
        AgentEvent::Done("last done".to_string()),
    ];
    assert_eq!(extract_final_message(&events), "last done");
}

// -- build_context -------------------------------------------------------

#[test]
fn test_build_context() {
    let task = ClusterTask {
        task_id: "task-001".to_string(),
        source: TaskSource {
            node_id: "node-b".to_string(),
            rpc_address: "192.168.1.10:9000".to_string(),
            session_key: "sess-abc".to_string(),
        },
        status: TaskStatus::Pending,
        content: "hello".to_string(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
    };
    let ctx = build_context(&task);
    assert_eq!(ctx.channel, "cluster");
    // chat_id 现在等于 session_key（稳定），不再拼 task_id
    assert_eq!(ctx.chat_id, "sess-abc");
    assert_eq!(ctx.user, "node-b");
    assert_eq!(ctx.session_key, "sess-abc");
    assert!(ctx.correlation_id.is_none());
}

// -- restore_session_history / persist_session_history -------------------

/// Minimal mock LLM provider so we can construct an AgentLoop without spinning
/// up the real provider stack. None of these tests actually call the LLM —
/// they only exercise the SessionStore glue in helpers.
struct NullLlmProvider;

#[async_trait::async_trait]
impl LlmProvider for NullLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "null".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// Build an AgentLoop with an in-memory SessionStore for testing.
fn make_loop_with_session_store() -> (AgentLoop, std::sync::Arc<SessionStore>) {
    let mut agent_loop = AgentLoop::new(
        Box::new(NullLlmProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
    );
    let store = std::sync::Arc::new(SessionStore::new_in_memory());
    agent_loop.set_session_store(store.clone());
    (agent_loop, store)
}

fn make_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test".to_string()),
        max_turns: 1,
        tools: vec![],
        ..Default::default()
    }
}

// -- degrade paths: missing SessionStore + save failure -------------------

/// Build an AgentLoop WITHOUT a SessionStore attached. Mirrors the production
/// path when `build_cluster_agent_loop` couldn't create the storage directory
/// (rare, but the code must degrade gracefully).
fn make_loop_without_session_store() -> AgentLoop {
    AgentLoop::new(
        Box::new(NullLlmProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
    )
}

#[test]
fn test_restore_silent_when_no_session_store_attached() {
    // session_store() == None path: restore_session_history must return 0
    // and not panic. Same for persist_session_history (covered by next test).
    let agent_loop = make_loop_without_session_store();
    let instance = AgentInstance::new(make_test_config());

    let restored = restore_session_history(&agent_loop, &instance, "any-key");
    assert_eq!(restored, 0);
    assert_eq!(instance.get_history().len(), 1); // system prompt only
}

#[test]
fn test_persist_silent_when_no_session_store_attached() {
    let agent_loop = make_loop_without_session_store();
    let instance = AgentInstance::new(make_test_config());

    // Must not panic; must not modify the instance.
    persist_session_history(&agent_loop, &instance, "any-key", "hello", "world");
    assert_eq!(instance.get_history().len(), 1);
}

#[test]
fn test_persist_save_failure_does_not_panic() {
    // Construct a disk-backed SessionStore, then trigger save() failure by
    // passing an invalid session key (".." sanitizes to "." which is rejected).
    // persist_session_history catches the Err and logs a warning instead of
    // propagating.
    let tmp = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(SessionStore::new_with_storage(tmp.path()));
    let mut agent_loop = AgentLoop::new(
        Box::new(NullLlmProvider),
        make_test_config(),
    );
    agent_loop.set_session_store(store.clone());

    let instance = AgentInstance::new(make_test_config());

    // Direct save("..") fails; verify the precondition holds.
    store.get_or_create("..");
    assert!(store.save("..").is_err());

    // persist_session_history with invalid key must swallow the error.
    persist_session_history(&agent_loop, &instance, "..", "hello", "world");

    // Instance history is untouched (persist doesn't read instance for
    // append-only writes — it just adds user/assistant rows).
    assert_eq!(instance.get_history().len(), 1);
}

#[test]
fn test_restore_session_history_empty_store_returns_zero() {
    let (agent_loop, _store) = make_loop_with_session_store();
    let instance = AgentInstance::new(make_test_config());

    let restored = restore_session_history(&agent_loop, &instance, "nonexistent-key");
    assert_eq!(restored, 0);
    // Fresh instance has 1 system message (from config); restore adds nothing.
    assert_eq!(instance.get_history().len(), 1);
    assert_eq!(instance.get_history()[0].role, "system");
}

#[test]
fn test_persist_then_restore_roundtrip() {
    let (agent_loop, store) = make_loop_with_session_store();
    let instance = AgentInstance::new(make_test_config());

    // Persist a (user, assistant) pair.
    persist_session_history(&agent_loop, &instance, "sess-1", "hello", "world");

    // Store should have 2 messages (no system prompt stored).
    let msgs = store.get_history("sess-1");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "hello");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "world");

    // New instance should be able to restore the same history.
    // set_history automatically prepends the instance's system_prompt, so the
    // final history is [system, user, assistant] = 3 turns.
    let instance2 = AgentInstance::new(make_test_config());
    let restored = restore_session_history(&agent_loop, &instance2, "sess-1");
    assert_eq!(restored, 2);
    let history = instance2.get_history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].role, "user");
    assert_eq!(history[1].content, "hello");
    assert_eq!(history[2].role, "assistant");
    assert_eq!(history[2].content, "world");
}

#[test]
fn test_different_session_keys_isolated() {
    let (agent_loop, store) = make_loop_with_session_store();
    let instance = AgentInstance::new(make_test_config());

    persist_session_history(&agent_loop, &instance, "sess-A", "hello-A", "world-A");
    persist_session_history(&agent_loop, &instance, "sess-B", "hello-B", "world-B");

    assert_eq!(store.get_history("sess-A").len(), 2);
    assert_eq!(store.get_history("sess-B").len(), 2);

    // Restoring A on a fresh instance must give A's messages, not B's.
    // history = [system, user-A, assistant-A]; we check indexes 1 and 2.
    let instance_a = AgentInstance::new(make_test_config());
    let restored = restore_session_history(&agent_loop, &instance_a, "sess-A");
    assert_eq!(restored, 2);
    let history = instance_a.get_history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].content, "hello-A");
    assert_eq!(history[2].content, "world-A");
}

#[test]
fn test_persist_appends_across_multiple_calls() {
    // Multi-turn conversation: each peer_chat appends a (user, assistant) pair.
    // Verifies we accumulate history rather than overwrite.
    let (agent_loop, store) = make_loop_with_session_store();
    let instance = AgentInstance::new(make_test_config());

    persist_session_history(&agent_loop, &instance, "sess-multi", "msg-1", "resp-1");
    persist_session_history(&agent_loop, &instance, "sess-multi", "msg-2", "resp-2");

    let msgs = store.get_history("sess-multi");
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "msg-1");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "resp-1");
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].content, "msg-2");
    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(msgs[3].content, "resp-2");
}

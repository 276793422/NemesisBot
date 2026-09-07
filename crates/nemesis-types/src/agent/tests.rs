use super::*;

// --- SessionKey ---

#[test]
fn test_session_key_new() {
    let sk = SessionKey::new("web", "chat123");
    assert_eq!(sk.0, "web:chat123");
}

#[test]
fn test_session_key_new_empty_parts() {
    let sk = SessionKey::new("", "");
    assert_eq!(sk.0, ":");
}

#[test]
fn test_session_key_new_with_colon_in_chat_id() {
    let sk = SessionKey::new("rpc", "host:1234");
    assert_eq!(sk.0, "rpc:host:1234");
}

#[test]
fn test_session_key_hash_eq() {
    let sk1 = SessionKey::new("web", "chat1");
    let sk2 = SessionKey::new("web", "chat1");
    assert_eq!(sk1, sk2);

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h1 = DefaultHasher::new();
    let mut h2 = DefaultHasher::new();
    sk1.hash(&mut h1);
    sk2.hash(&mut h2);
    assert_eq!(h1.finish(), h2.finish());
}

#[test]
fn test_session_key_ne() {
    let sk1 = SessionKey::new("web", "chat1");
    let sk2 = SessionKey::new("rpc", "chat1");
    assert_ne!(sk1, sk2);
}

#[test]
fn test_session_key_clone() {
    let sk1 = SessionKey::new("web", "chat1");
    let sk2 = sk1.clone();
    assert_eq!(sk1, sk2);
}

#[test]
fn test_session_key_debug() {
    let sk = SessionKey::new("web", "chat1");
    let dbg = format!("{:?}", sk);
    assert!(dbg.contains("web:chat1"));
}

#[test]
fn test_session_key_serialize_deserialize() {
    let sk = SessionKey::new("web", "chat1");
    let json = serde_json::to_string(&sk).unwrap();
    let sk2: SessionKey = serde_json::from_str(&json).unwrap();
    assert_eq!(sk, sk2);
}

#[test]
fn test_session_key_in_hashset() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(SessionKey::new("web", "chat1"));
    set.insert(SessionKey::new("web", "chat1")); // duplicate
    set.insert(SessionKey::new("rpc", "chat1"));
    assert_eq!(set.len(), 2);
}

#[test]
fn test_session_key_in_hashmap() {
    use std::collections::HashMap;
    let mut map = HashMap::new();
    map.insert(SessionKey::new("web", "chat1"), "value1");
    assert_eq!(map.get(&SessionKey::new("web", "chat1")), Some(&"value1"));
    assert_eq!(map.get(&SessionKey::new("rpc", "chat1")), None);
}

// --- AgentConfig ---

#[test]
fn test_agent_config_default() {
    let cfg = AgentConfig::default();
    assert_eq!(cfg.max_iterations, 60);
    assert_eq!(cfg.max_context_tokens, 128000);
    assert!(cfg.system_prompt.is_none());
    assert!((cfg.temperature - 0.7).abs() < f64::EPSILON);
    assert!((cfg.top_p - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_agent_config_serialize_deserialize() {
    let cfg = AgentConfig::default();
    let json = serde_json::to_string(&cfg).unwrap();
    let cfg2: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg.max_iterations, cfg2.max_iterations);
    assert_eq!(cfg.max_context_tokens, cfg2.max_context_tokens);
    assert_eq!(cfg.system_prompt, cfg2.system_prompt);
    assert!((cfg.temperature - cfg2.temperature).abs() < f64::EPSILON);
    assert!((cfg.top_p - cfg2.top_p).abs() < f64::EPSILON);
}

#[test]
fn test_agent_config_with_custom_values() {
    let json = r#"{
        "max_iterations": 20,
        "max_context_tokens": 256000,
        "system_prompt": "You are helpful",
        "temperature": 0.5,
        "top_p": 0.9
    }"#;
    let cfg: AgentConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.max_iterations, 20);
    assert_eq!(cfg.max_context_tokens, 256000);
    assert_eq!(cfg.system_prompt, Some("You are helpful".to_string()));
    assert!((cfg.temperature - 0.5).abs() < f64::EPSILON);
    assert!((cfg.top_p - 0.9).abs() < f64::EPSILON);
}

#[test]
fn test_agent_config_clone() {
    let cfg = AgentConfig::default();
    let cfg2 = cfg.clone();
    assert_eq!(cfg.max_iterations, cfg2.max_iterations);
}

// --- MessageRole ---

#[test]
fn test_message_role_equality() {
    assert_eq!(MessageRole::System, MessageRole::System);
    assert_eq!(MessageRole::User, MessageRole::User);
    assert_eq!(MessageRole::Assistant, MessageRole::Assistant);
    assert_eq!(MessageRole::Tool, MessageRole::Tool);
}

#[test]
fn test_message_role_inequality() {
    assert_ne!(MessageRole::System, MessageRole::User);
    assert_ne!(MessageRole::Assistant, MessageRole::Tool);
    assert_ne!(MessageRole::User, MessageRole::Assistant);
}

#[test]
fn test_message_role_serialize_deserialize() {
    let roles = vec![
        MessageRole::System,
        MessageRole::User,
        MessageRole::Assistant,
        MessageRole::Tool,
    ];
    let json = serde_json::to_string(&roles).unwrap();
    let roles2: Vec<MessageRole> = serde_json::from_str(&json).unwrap();
    assert_eq!(roles, roles2);
}

#[test]
fn test_message_role_json_values() {
    assert_eq!(
        serde_json::to_string(&MessageRole::System).unwrap(),
        "\"System\""
    );
    assert_eq!(
        serde_json::to_string(&MessageRole::User).unwrap(),
        "\"User\""
    );
    assert_eq!(
        serde_json::to_string(&MessageRole::Assistant).unwrap(),
        "\"Assistant\""
    );
    assert_eq!(
        serde_json::to_string(&MessageRole::Tool).unwrap(),
        "\"Tool\""
    );
}

// --- AgentMessage ---

#[test]
fn test_agent_message_basic() {
    let msg = AgentMessage {
        role: MessageRole::User,
        content: "Hello".to_string(),
        tool_calls: None,
        tool_call_id: None,
    };
    assert_eq!(msg.role, MessageRole::User);
    assert_eq!(msg.content, "Hello");
    assert!(msg.tool_calls.is_none());
    assert!(msg.tool_call_id.is_none());
}

#[test]
fn test_agent_message_with_tool_calls() {
    let msg = AgentMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        tool_calls: Some(vec![ToolCall {
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        }]),
        tool_call_id: None,
    };
    assert_eq!(msg.role, MessageRole::Assistant);
    let tc = msg.tool_calls.as_ref().unwrap();
    assert_eq!(tc.len(), 1);
    assert_eq!(tc[0].id, "tc_1");
    assert_eq!(tc[0].name, "read_file");
    assert_eq!(tc[0].arguments["path"], "/tmp/test.txt");
}

#[test]
fn test_agent_message_tool_result() {
    let msg = AgentMessage {
        role: MessageRole::Tool,
        content: "file contents here".to_string(),
        tool_calls: None,
        tool_call_id: Some("tc_1".to_string()),
    };
    assert_eq!(msg.tool_call_id, Some("tc_1".to_string()));
}

#[test]
fn test_agent_message_serialize_deserialize() {
    let msg = AgentMessage {
        role: MessageRole::User,
        content: "Hello world".to_string(),
        tool_calls: None,
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.role, msg2.role);
    assert_eq!(msg.content, msg2.content);
}

#[test]
fn test_agent_message_roundtrip_with_tools() {
    let msg = AgentMessage {
        role: MessageRole::Assistant,
        content: "".to_string(),
        tool_calls: Some(vec![
            ToolCall {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            },
            ToolCall {
                id: "call_2".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "test.txt"}),
            },
        ]),
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg2.tool_calls.as_ref().unwrap().len(), 2);
    assert_eq!(msg2.tool_calls.as_ref().unwrap()[0].name, "bash");
    assert_eq!(msg2.tool_calls.as_ref().unwrap()[1].name, "read_file");
}

// --- ToolCall ---

#[test]
fn test_tool_call_serialize_deserialize() {
    let tc = ToolCall {
        id: "call_abc".to_string(),
        name: "execute".to_string(),
        arguments: serde_json::json!({"cmd": "echo hello", "timeout": 30}),
    };
    let json = serde_json::to_string(&tc).unwrap();
    let tc2: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(tc.id, tc2.id);
    assert_eq!(tc.name, tc2.name);
    assert_eq!(tc.arguments, tc2.arguments);
}

#[test]
fn test_tool_call_empty_arguments() {
    let tc = ToolCall {
        id: "call_1".to_string(),
        name: "ping".to_string(),
        arguments: serde_json::json!(null),
    };
    let json = serde_json::to_string(&tc).unwrap();
    let tc2: ToolCall = serde_json::from_str(&json).unwrap();
    assert!(tc2.arguments.is_null());
}

#[test]
fn test_tool_call_clone() {
    let tc = ToolCall {
        id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({}),
    };
    let tc2 = tc.clone();
    assert_eq!(tc.id, tc2.id);
    assert_eq!(tc.name, tc2.name);
}

// --- ToolResult ---

#[test]
fn test_tool_result_success() {
    let tr = ToolResult {
        tool_call_id: "tc_1".to_string(),
        content: "output here".to_string(),
        is_error: false,
    };
    assert!(!tr.is_error);
    assert_eq!(tr.content, "output here");
}

#[test]
fn test_tool_result_error() {
    let tr = ToolResult {
        tool_call_id: "tc_2".to_string(),
        content: "file not found".to_string(),
        is_error: true,
    };
    assert!(tr.is_error);
}

#[test]
fn test_tool_result_serialize_deserialize() {
    let tr = ToolResult {
        tool_call_id: "tc_1".to_string(),
        content: "result".to_string(),
        is_error: false,
    };
    let json = serde_json::to_string(&tr).unwrap();
    let tr2: ToolResult = serde_json::from_str(&json).unwrap();
    assert_eq!(tr.tool_call_id, tr2.tool_call_id);
    assert_eq!(tr.content, tr2.content);
    assert_eq!(tr.is_error, tr2.is_error);
}

#[test]
fn test_tool_result_clone() {
    let tr = ToolResult {
        tool_call_id: "tc_1".to_string(),
        content: "data".to_string(),
        is_error: true,
    };
    let tr2 = tr.clone();
    assert_eq!(tr.tool_call_id, tr2.tool_call_id);
    assert_eq!(tr.content, tr2.content);
    assert_eq!(tr.is_error, tr2.is_error);
}

// --- AgentSession ---

#[test]
fn test_agent_session_basic() {
    let session = AgentSession {
        session_key: SessionKey::new("web", "chat1"),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        messages: vec![],
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    };
    assert_eq!(session.channel, "web");
    assert_eq!(session.chat_id, "chat1");
    assert!(session.messages.is_empty());
}

#[test]
fn test_agent_session_with_messages() {
    let session = AgentSession {
        session_key: SessionKey::new("web", "chat1"),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        messages: vec![
            AgentMessage {
                role: MessageRole::System,
                content: "You are helpful".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            AgentMessage {
                role: MessageRole::User,
                content: "Hi".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            AgentMessage {
                role: MessageRole::Assistant,
                content: "Hello!".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
        ],
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:01:00Z".to_string(),
    };
    assert_eq!(session.messages.len(), 3);
    assert_eq!(session.messages[0].role, MessageRole::System);
    assert_eq!(session.messages[1].role, MessageRole::User);
    assert_eq!(session.messages[2].role, MessageRole::Assistant);
}

#[test]
fn test_agent_session_serialize_deserialize() {
    let session = AgentSession {
        session_key: SessionKey::new("rpc", "chat42"),
        channel: "rpc".to_string(),
        chat_id: "chat42".to_string(),
        messages: vec![AgentMessage {
            role: MessageRole::User,
            content: "test".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }],
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&session).unwrap();
    let session2: AgentSession = serde_json::from_str(&json).unwrap();
    assert_eq!(session.channel, session2.channel);
    assert_eq!(session.chat_id, session2.chat_id);
    assert_eq!(session.messages.len(), session2.messages.len());
    assert_eq!(session.session_key, session2.session_key);
}

#[test]
fn test_agent_session_clone() {
    let session = AgentSession {
        session_key: SessionKey::new("web", "c1"),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        messages: vec![],
        created_at: "t1".to_string(),
        updated_at: "t2".to_string(),
    };
    let session2 = session.clone();
    assert_eq!(session.session_key, session2.session_key);
    assert_eq!(session.channel, session2.channel);
}

// ---------------------------------------------------------------------------
// M7（devtool-upgrade 阶段 5）：ApprovalRequested 事件 + ApprovalResponder trait
// ---------------------------------------------------------------------------

#[test]
fn test_agent_event_approval_requested_serde_tag_shape() {
    use super::AgentEvent;
    let ev = AgentEvent::ApprovalRequested {
        session_key: "agent:main:session:s1".to_string(),
        chat_id: "web:abc".to_string(),
        request_id: "req-1".to_string(),
        operation: "process_exec".to_string(),
        target: "cargo publish".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "rule: exec-publish".to_string(),
        timeout_secs: 300,
        pattern: "cargo publish *".to_string(),
    };
    let v = serde_json::to_value(&ev).unwrap();
    // serde(tag = "kind", content = "data") 形状——web pump 原样转 SSE/WS。
    // tag 取变体原名（enum 无 rename_all，与 ModeChanged 等既有 kind 一致）。
    assert_eq!(v["kind"], "ApprovalRequested");
    assert_eq!(v["data"]["request_id"], "req-1");
    assert_eq!(v["data"]["operation"], "process_exec");
    assert_eq!(v["data"]["risk_level"], "HIGH");
    assert_eq!(v["data"]["timeout_secs"], 300);
    assert_eq!(v["data"]["pattern"], "cargo publish *");
    // 反序列化回事件并逐字段核对（AgentEvent 未 derive PartialEq）。
    match serde_json::from_value::<AgentEvent>(v).unwrap() {
        AgentEvent::ApprovalRequested {
            session_key,
            chat_id,
            request_id,
            operation,
            target,
            risk_level,
            reason,
            timeout_secs,
            pattern,
        } => {
            assert_eq!(session_key, "agent:main:session:s1");
            assert_eq!(chat_id, "web:abc");
            assert_eq!(request_id, "req-1");
            assert_eq!(operation, "process_exec");
            assert_eq!(target, "cargo publish");
            assert_eq!(risk_level, "HIGH");
            assert_eq!(reason, "rule: exec-publish");
            assert_eq!(timeout_secs, 300);
            assert_eq!(pattern, "cargo publish *");
        }
        other => panic!("wrong variant: {:?}", other),
    }
}

#[test]
fn test_agent_event_approval_requested_accessors() {
    use super::AgentEvent;
    let ev = AgentEvent::ApprovalRequested {
        session_key: "k".to_string(),
        chat_id: "web:x".to_string(),
        request_id: "r".to_string(),
        operation: "file_write".to_string(),
        target: "t".to_string(),
        risk_level: "CRITICAL".to_string(),
        reason: "why".to_string(),
        timeout_secs: 60,
        pattern: String::new(),
    };
    assert_eq!(ev.chat_id(), "web:x");
    assert_eq!(ev.kind(), "ApprovalRequested");
}

#[test]
fn test_approval_responder_is_object_safe() {
    use super::ApprovalResponder;
    struct Dummy;
    impl ApprovalResponder for Dummy {
        fn respond(
            &self,
            _request_id: &str,
            _approved: bool,
            _always: bool,
            _note: Option<String>,
        ) -> Result<bool, String> {
            Ok(true)
        }
        fn pending(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }
    }
    // trait 对象可持有（nemesis-web 经 agent_loop 槽触达的形态）。
    let r: std::sync::Arc<dyn ApprovalResponder> = std::sync::Arc::new(Dummy);
    assert!(r.respond("x", true, false, None).unwrap());
    assert!(r.pending().is_empty());
}

#[test]
fn test_agent_event_approval_resolved_serde_shape() {
    use super::AgentEvent;
    let ev = AgentEvent::ApprovalResolved {
        request_id: "req-9".to_string(),
        decision: "denied".to_string(),
    };
    let v = serde_json::to_value(&ev).unwrap();
    // 与 ApprovalRequested 同形状：tag=变体原名，data 承载字段。
    assert_eq!(v["kind"], "ApprovalResolved");
    assert_eq!(v["data"]["request_id"], "req-9");
    assert_eq!(v["data"]["decision"], "denied");
    // 反序列化回事件逐字段核对。
    match serde_json::from_value::<AgentEvent>(v).unwrap() {
        AgentEvent::ApprovalResolved {
            request_id,
            decision,
        } => {
            assert_eq!(request_id, "req-9");
            assert_eq!(decision, "denied");
        }
        other => panic!("wrong variant: {:?}", other),
    }
    // 全局事件：chat_id 恒空串（不属单一会话，同 ApprovalRequested）。
    let ev = AgentEvent::ApprovalResolved {
        request_id: "r".to_string(),
        decision: "timeout".to_string(),
    };
    assert_eq!(ev.chat_id(), "");
    assert_eq!(ev.kind(), "ApprovalResolved");
}

/// F7（devtool-upgrade 阶段 5）：QuestionAsked / QuestionResolved 的 serde
/// 形状 + accessors。QuestionAsked 带会话 chat_id；Resolved 是全局事件。
#[test]
fn test_agent_event_question_serde_shape() {
    use super::{AgentEvent, QuestionOutcome, QuestionRequest};

    let ev = AgentEvent::QuestionAsked {
        session_key: "web:chat-1".to_string(),
        chat_id: "chat-1".to_string(),
        question_id: "q-3".to_string(),
        question: "用哪个包管理器?".to_string(),
        options: vec!["pnpm".to_string(), "npm".to_string()],
        multi: false,
        timeout_secs: 120,
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["kind"], "QuestionAsked");
    assert_eq!(v["data"]["question_id"], "q-3");
    assert_eq!(v["data"]["question"], "用哪个包管理器?");
    assert_eq!(v["data"]["options"][0], "pnpm");
    assert_eq!(v["data"]["multi"], false);
    assert_eq!(v["data"]["timeout_secs"], 120);
    match serde_json::from_value::<AgentEvent>(v).unwrap() {
        AgentEvent::QuestionAsked {
            question_id,
            options,
            multi,
            ..
        } => {
            assert_eq!(question_id, "q-3");
            assert_eq!(options, vec!["pnpm".to_string(), "npm".to_string()]);
            assert!(!multi);
        }
        other => panic!("wrong variant: {:?}", other),
    }
    // 提问有会话上下文：chat_id 透传。
    assert_eq!(ev.chat_id(), "chat-1");
    assert_eq!(ev.kind(), "QuestionAsked");

    let resolved = AgentEvent::QuestionResolved {
        question_id: "q-3".to_string(),
        decision: "answered".to_string(),
    };
    let v = serde_json::to_value(&resolved).unwrap();
    assert_eq!(v["kind"], "QuestionResolved");
    assert_eq!(v["data"]["question_id"], "q-3");
    assert_eq!(v["data"]["decision"], "answered");
    assert_eq!(resolved.chat_id(), "");
    assert_eq!(resolved.kind(), "QuestionResolved");

    // QuestionOutcome：单选恰一项 / Timeout 与 Answered 可区分。
    let answered = QuestionOutcome::Answered(vec!["pnpm".to_string()]);
    assert_eq!(
        answered,
        QuestionOutcome::Answered(vec!["pnpm".to_string()])
    );
    assert_ne!(answered, QuestionOutcome::Timeout);
    let req = QuestionRequest {
        question_id: "q-1".to_string(),
        question: "q".to_string(),
        options: vec!["a".to_string(), "b".to_string()],
        multi: true,
        chat_id: "c".to_string(),
        session_key: "s".to_string(),
        timeout_secs: 60,
    };
    assert_eq!(req.timeout_secs, 60);
}

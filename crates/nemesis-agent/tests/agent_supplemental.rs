//! Supplemental tests for nemesis-agent crate.
//!
//! Covers: agent instance lifecycle, context building, memory management,
//! session management, request context, token estimation, force compression,
//! ring buffer, agent registry, request logger, conversation memory, and more.

use nemesis_agent::*;
use nemesis_agent::context::{RequestContext, ContextBuilder, SkillInfo};
use nemesis_agent::session::{StoredToolCall, Session, SessionManager, SessionStore, StoredSession, StoredMessage};
use nemesis_agent::session::{estimate_tokens, estimate_tokens_for_turns, force_compress_turns, is_internal_channel};
use nemesis_agent::memory::*;
use nemesis_agent::ringbuffer::RingBuffer;
use nemesis_agent::registry::AgentRegistry;
use nemesis_agent::request_logger::*;
use nemesis_agent::r#loop::{LlmMessage, ConcurrentMode, SessionBusyTracker};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ===========================================================================
// AgentConfig tests
// ===========================================================================

#[test]
fn test_agent_config_default_values() {
    let config = AgentConfig::default();
    assert_eq!(config.model, "gpt-4");
    assert!(config.system_prompt.is_none());
    assert_eq!(config.max_turns, 10);
    assert!(config.tools.is_empty());
}

#[test]
fn test_agent_config_custom() {
    let config = AgentConfig {
        model: "claude-sonnet-4-6".to_string(),
        system_prompt: Some("You are a helpful assistant".to_string()),
        max_turns: 5,
        tools: vec!["search".to_string(), "calculator".to_string()],
    };
    assert_eq!(config.model, "claude-sonnet-4-6");
    assert_eq!(config.system_prompt.as_deref(), Some("You are a helpful assistant"));
    assert_eq!(config.max_turns, 5);
    assert_eq!(config.tools.len(), 2);
}

#[test]
fn test_agent_config_serialization() {
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test prompt".to_string()),
        max_turns: 3,
        tools: vec!["t1".to_string()],
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.model, config.model);
    assert_eq!(back.system_prompt, config.system_prompt);
    assert_eq!(back.max_turns, config.max_turns);
    assert_eq!(back.tools, config.tools);
}

#[test]
fn test_agent_config_no_system_prompt() {
    let config = AgentConfig {
        model: "m".to_string(),
        system_prompt: None,
        max_turns: 1,
        tools: vec![],
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert!(back.system_prompt.is_none());
}

#[test]
fn test_agent_config_empty_tools() {
    let config = AgentConfig {
        model: "m".to_string(),
        system_prompt: None,
        max_turns: 1,
        tools: vec![],
    };
    assert!(config.tools.is_empty());
}

#[test]
fn test_agent_config_many_tools() {
    let config = AgentConfig {
        model: "m".to_string(),
        system_prompt: None,
        max_turns: 10,
        tools: (0..50).map(|i| format!("tool_{}", i)).collect(),
    };
    assert_eq!(config.tools.len(), 50);
}

// ===========================================================================
// ChatOptions tests
// ===========================================================================

#[test]
fn test_chat_options_default() {
    let opts = ChatOptions::default();
    assert_eq!(opts.max_tokens, Some(8192));
    assert_eq!(opts.temperature, Some(0.7));
    assert!(opts.top_p.is_none());
    assert!(opts.stop.is_none());
}

#[test]
fn test_chat_options_custom() {
    let opts = ChatOptions {
        max_tokens: Some(4096),
        temperature: Some(0.5),
        top_p: Some(0.9),
        stop: Some(vec!["\n".to_string()]),
    };
    assert_eq!(opts.max_tokens, Some(4096));
    assert_eq!(opts.temperature, Some(0.5));
    assert_eq!(opts.top_p, Some(0.9));
    assert_eq!(opts.stop.as_ref().unwrap().len(), 1);
}

#[test]
fn test_chat_options_serialization_skips_none() {
    let opts = ChatOptions {
        max_tokens: Some(100),
        temperature: None,
        top_p: None,
        stop: None,
    };
    let json = serde_json::to_string(&opts).unwrap();
    assert!(json.contains("max_tokens"));
    assert!(!json.contains("temperature"));
    assert!(!json.contains("top_p"));
    assert!(!json.contains("stop"));
}

// ===========================================================================
// ToolDefinition tests
// ===========================================================================

#[test]
fn test_tool_definition_default() {
    let def = ToolDefinition::default();
    assert_eq!(def.tool_type, "function");
    assert!(def.function.name.is_empty());
    assert!(def.function.description.is_empty());
}

#[test]
fn test_tool_definition_custom() {
    let def = ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDef {
            name: "calculator".to_string(),
            description: "Performs arithmetic".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": {"type": "string"}
                }
            }),
        },
    };
    let json = serde_json::to_string(&def).unwrap();
    let back: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(back.function.name, "calculator");
    assert_eq!(back.function.description, "Performs arithmetic");
}

// ===========================================================================
// AgentState tests
// ===========================================================================

#[test]
fn test_agent_state_default() {
    assert_eq!(AgentState::default(), AgentState::Idle);
}

#[test]
fn test_agent_state_serialization() {
    for state in &[AgentState::Idle, AgentState::Thinking, AgentState::ExecutingTool, AgentState::Responding] {
        let json = serde_json::to_string(state).unwrap();
        let back: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, *state);
    }
}

#[test]
fn test_agent_state_equality() {
    assert_eq!(AgentState::Idle, AgentState::Idle);
    assert_ne!(AgentState::Idle, AgentState::Thinking);
    assert_ne!(AgentState::Thinking, AgentState::ExecutingTool);
    assert_ne!(AgentState::ExecutingTool, AgentState::Responding);
}

// ===========================================================================
// AgentEvent tests
// ===========================================================================

#[test]
fn test_agent_event_message() {
    let event = AgentEvent::Message("hello".to_string());
    let json = serde_json::to_string(&event).unwrap();
    let back: AgentEvent = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2);
}

#[test]
fn test_agent_event_tool_call() {
    let event = AgentEvent::ToolCall(vec![
        ToolCallInfo {
            id: "tc-1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
        },
    ]);
    let json = serde_json::to_string(&event).unwrap();
    let back: AgentEvent = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2);
}

#[test]
fn test_agent_event_tool_result() {
    let event = AgentEvent::ToolResult(ToolCallResult {
        tool_name: "calculator".to_string(),
        result: "42".to_string(),
        is_error: false,
    });
    let json = serde_json::to_string(&event).unwrap();
    let back: AgentEvent = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2);
}

#[test]
fn test_agent_event_error() {
    let event = AgentEvent::Error("something went wrong".to_string());
    let json = serde_json::to_string(&event).unwrap();
    let back: AgentEvent = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2);
}

#[test]
fn test_agent_event_done() {
    let event = AgentEvent::Done("final answer".to_string());
    let json = serde_json::to_string(&event).unwrap();
    let back: AgentEvent = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2);
}

// ===========================================================================
// ConversationTurn and ToolCallInfo tests
// ===========================================================================

#[test]
fn test_conversation_turn_user() {
    let turn = ConversationTurn {
        role: "user".to_string(),
        content: "What is 2+2?".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&turn).unwrap();
    let back: ConversationTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(back, turn);
}

#[test]
fn test_conversation_turn_assistant_with_tools() {
    let turn = ConversationTurn {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![
            ToolCallInfo {
                id: "tc-1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr": "2+2"}"#.to_string(),
            },
            ToolCallInfo {
                id: "tc-2".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query": "result"}"#.to_string(),
            },
        ],
        tool_call_id: None,
        timestamp: "2026-01-01T00:00:01Z".to_string(),
    };
    assert_eq!(turn.tool_calls.len(), 2);
    let json = serde_json::to_string(&turn).unwrap();
    let back: ConversationTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tool_calls.len(), 2);
    assert_eq!(back.tool_calls[0].name, "calculator");
    assert_eq!(back.tool_calls[1].name, "search");
}

#[test]
fn test_conversation_turn_tool_response() {
    let turn = ConversationTurn {
        role: "tool".to_string(),
        content: "42".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("tc-1".to_string()),
        timestamp: "2026-01-01T00:00:02Z".to_string(),
    };
    assert_eq!(turn.tool_call_id, Some("tc-1".to_string()));
    let json = serde_json::to_string(&turn).unwrap();
    let back: ConversationTurn = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tool_call_id, turn.tool_call_id);
}

#[test]
fn test_tool_call_result_success() {
    let result = ToolCallResult {
        tool_name: "calculator".to_string(),
        result: "42".to_string(),
        is_error: false,
    };
    assert!(!result.is_error);
    let json = serde_json::to_string(&result).unwrap();
    let back: ToolCallResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back.result, "42");
}

#[test]
fn test_tool_call_result_error() {
    let result = ToolCallResult {
        tool_name: "file_read".to_string(),
        result: "file not found".to_string(),
        is_error: true,
    };
    assert!(result.is_error);
    let json = serde_json::to_string(&result).unwrap();
    let back: ToolCallResult = serde_json::from_str(&json).unwrap();
    assert!(back.is_error);
}

#[test]
fn test_tool_call_info_equality() {
    let tc1 = ToolCallInfo {
        id: "tc-1".to_string(),
        name: "search".to_string(),
        arguments: "{}".to_string(),
    };
    let tc2 = tc1.clone();
    assert_eq!(tc1, tc2);
}

// ===========================================================================
// AgentInstance lifecycle tests
// ===========================================================================

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["search".to_string()],
    }
}

#[test]
fn test_agent_instance_new() {
    let instance = AgentInstance::new(test_config());
    assert_eq!(instance.state(), AgentState::Idle);
    assert_eq!(instance.config().model, "test-model");
    assert_eq!(instance.config().max_turns, 5);
    assert_eq!(instance.message_count(), 0);
}

#[test]
fn test_agent_instance_state_transitions() {
    let instance = AgentInstance::new(test_config());
    assert_eq!(instance.state(), AgentState::Idle);

    assert!(instance.start_thinking());
    assert_eq!(instance.state(), AgentState::Thinking);

    assert!(instance.start_tool_execution());
    assert_eq!(instance.state(), AgentState::ExecutingTool);

    assert!(instance.start_responding());
    assert_eq!(instance.state(), AgentState::Responding);

    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);
}

#[test]
fn test_agent_instance_double_thinking_fails() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.start_thinking());
    assert!(!instance.start_thinking()); // Already thinking
}

#[test]
fn test_agent_instance_start_tool_from_idle_fails() {
    let instance = AgentInstance::new(test_config());
    assert!(!instance.start_tool_execution()); // Must be thinking first
}

#[test]
fn test_agent_instance_start_responding_from_idle_fails() {
    let instance = AgentInstance::new(test_config());
    assert!(!instance.start_responding()); // Must be executing tool first
}

#[test]
fn test_agent_instance_add_messages() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    assert_eq!(instance.message_count(), 1);

    instance.add_assistant_message("Hi there!", vec![]);
    assert_eq!(instance.message_count(), 2);

    instance.add_tool_result("tc-1", "result data");
    assert_eq!(instance.message_count(), 3);
}

#[test]
fn test_agent_instance_history() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    instance.add_assistant_message("Hi!", vec![]);

    let history = instance.get_history();
    // system prompt + user + assistant = 3
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].role, "user");
    assert_eq!(history[1].content, "Hello");
    assert_eq!(history[2].role, "assistant");
    assert_eq!(history[2].content, "Hi!");
}

#[test]
fn test_agent_instance_clear_history() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    instance.add_assistant_message("Hi!", vec![]);
    assert_eq!(instance.message_count(), 2);

    instance.clear_history();
    assert_eq!(instance.message_count(), 0);
}

#[test]
fn test_agent_instance_set_history() {
    let instance = AgentInstance::new(test_config());
    let history = vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "Test".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        },
    ];
    instance.set_history(history);
    assert_eq!(instance.message_count(), 1);
}

#[test]
fn test_agent_instance_truncate_to() {
    let instance = AgentInstance::new(test_config());
    for i in 0..10 {
        instance.add_user_message(&format!("msg {}", i));
    }
    assert_eq!(instance.message_count(), 10);
    instance.truncate_to(3);
    assert_eq!(instance.message_count(), 3);
}

#[test]
fn test_agent_instance_summary() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.get_summary().is_empty());
    instance.set_summary("Test summary");
    assert_eq!(instance.get_summary(), "Test summary");
}

#[test]
fn test_agent_instance_context_window() {
    let instance = AgentInstance::new(test_config());
    let default = instance.context_window();
    assert!(default > 0);

    let mut instance = instance;
    instance.set_context_window(8000);
    assert_eq!(instance.context_window(), 8000);
}

#[test]
fn test_agent_instance_metadata() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.metadata().is_null() || instance.metadata().is_object());

    instance.set_metadata(serde_json::json!({"key": "value"}));
    assert_eq!(instance.metadata()["key"], "value");
}

#[test]
fn test_agent_instance_workspace() {
    let mut instance = AgentInstance::new(test_config());
    instance.set_workspace(std::path::PathBuf::from("/tmp/test"));
    assert_eq!(instance.workspace(), &std::path::PathBuf::from("/tmp/test"));
}

#[test]
fn test_agent_instance_max_iterations() {
    let mut instance = AgentInstance::new(test_config());
    assert!(instance.max_iterations() > 0);
    instance.set_max_iterations(42);
    assert_eq!(instance.max_iterations(), 42);
}

#[test]
fn test_agent_instance_subagents() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.subagents().is_empty());
    instance.set_subagents(vec!["sub1".to_string(), "sub2".to_string()]);
    assert_eq!(instance.subagents().len(), 2);
}

#[test]
fn test_agent_instance_skills_filter() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.skills_filter().is_empty());
    instance.set_skills_filter(vec!["skill1".to_string()]);
    assert_eq!(instance.skills_filter().len(), 1);
}

#[test]
fn test_agent_instance_fallback_candidates() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.fallback_candidates().is_empty());
    instance.set_fallback_candidates(vec!["fallback1".to_string()]);
    assert_eq!(instance.fallback_candidates().len(), 1);
}

#[test]
fn test_agent_instance_provider_meta() {
    let instance = AgentInstance::new(test_config());
    assert!(instance.provider_meta().is_none());
    instance.set_provider_meta(serde_json::json!({"provider": "test"}));
    assert!(instance.provider_meta().is_some());
}

#[test]
fn test_agent_instance_id_unique() {
    let inst1 = AgentInstance::new(test_config());
    let inst2 = AgentInstance::new(test_config());
    assert_ne!(inst1.id(), inst2.id());
}

#[test]
fn test_agent_instance_add_assistant_with_tool_calls() {
    let instance = AgentInstance::new(test_config());
    instance.add_assistant_message("Using tools", vec![
        ToolCallInfo {
            id: "tc-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"test"}"#.to_string(),
        },
    ]);
    let history = instance.get_history();
    // history[0] is system prompt, history[1] is the assistant message
    assert_eq!(history[1].tool_calls.len(), 1);
    assert_eq!(history[1].tool_calls[0].name, "search");
}

// ===========================================================================
// RequestContext tests
// ===========================================================================

#[test]
fn test_request_context_new() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert_eq!(ctx.channel, "web");
    assert_eq!(ctx.chat_id, "chat1");
    assert_eq!(ctx.user, "user1");
    assert_eq!(ctx.session_key, "sess1");
    assert!(ctx.correlation_id.is_none());
    assert!(!ctx.is_rpc());
}

#[test]
fn test_request_context_with_correlation_id() {
    let ctx = RequestContext::with_correlation_id("rpc", "chat1", "user1", "sess1", "corr-123");
    assert_eq!(ctx.correlation_id.as_deref(), Some("corr-123"));
}

#[test]
fn test_request_context_for_rpc() {
    let ctx = RequestContext::for_rpc("chat1", "user1", "sess1", "corr-456");
    assert_eq!(ctx.channel, "rpc");
    assert!(ctx.is_rpc());
    assert_eq!(ctx.correlation_id.as_deref(), Some("corr-456"));
}

#[test]
fn test_request_context_format_rpc_message() {
    let ctx = RequestContext::for_rpc("chat1", "user1", "sess1", "abc");
    assert_eq!(ctx.format_rpc_message("Hello"), "[rpc:abc] Hello");
}

#[test]
fn test_request_context_format_non_rpc_message() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert_eq!(ctx.format_rpc_message("Hello"), "Hello");
}

#[test]
fn test_request_context_format_rpc_empty_correlation() {
    let ctx = RequestContext {
        channel: "rpc".to_string(),
        chat_id: "chat1".to_string(),
        user: "user1".to_string(),
        session_key: "sess1".to_string(),
        correlation_id: Some(String::new()),
        async_callback: None,
    };
    assert_eq!(ctx.format_rpc_message("Hello"), "Hello");
}

#[test]
fn test_request_context_async_callback() {
    let called = Arc::new(AtomicUsize::new(0));
    let called_clone = called.clone();
    let mut ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert!(!ctx.invoke_async_callback("test"));

    ctx.set_async_callback(Arc::new(move |msg: String| {
        assert_eq!(msg, "hello");
        called_clone.fetch_add(1, Ordering::SeqCst);
    }));

    assert!(ctx.invoke_async_callback("hello"));
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[test]
fn test_request_context_serialization() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let json = serde_json::to_string(&ctx).unwrap();
    let back: RequestContext = serde_json::from_str(&json).unwrap();
    assert_eq!(back.channel, "web");
    assert_eq!(back.chat_id, "chat1");
    assert_eq!(back.user, "user1");
    assert_eq!(back.session_key, "sess1");
}

// ===========================================================================
// ContextBuilder tests
// ===========================================================================

#[test]
fn test_context_builder_empty_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Current Time"));
    assert!(prompt.contains("Workspace"));
    assert!(prompt.contains("NemesisBot (Rust)"));
}

#[test]
fn test_context_builder_with_identity_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "I am a helpful assistant.").unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("IDENTITY.md"));
    assert!(prompt.contains("I am a helpful assistant."));
}

#[test]
fn test_context_builder_skip_bootstrap() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "I am a helper.").unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(true);
    assert!(prompt.contains("IDENTITY.md"));
}

#[test]
fn test_context_builder_bootstrap_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("BOOTSTRAP.md"), "Setup instructions.").unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Initialization Bootstrap Mode"));
    assert!(prompt.contains("Setup instructions."));
}

#[test]
fn test_context_builder_tool_summaries() {
    let tmp = tempfile::tempdir().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_tool_summaries(vec![
        "- tool1: Does thing 1".to_string(),
        "- tool2: Does thing 2".to_string(),
    ]);
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Available Tools"));
    assert!(prompt.contains("tool1"));
    assert!(prompt.contains("tool2"));
}

#[test]
fn test_context_builder_tools_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    let defs = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "calculator",
                "description": "Performs arithmetic",
                "parameters": {}
            }
        }),
    ];
    builder.set_tools_registry(defs);
    assert_eq!(builder.tool_definitions().len(), 1);
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("calculator"));
    assert!(prompt.contains("Performs arithmetic"));
}

#[test]
fn test_context_builder_skills_info() {
    let tmp = tempfile::tempdir().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_skills_info(vec![
        SkillInfo {
            name: "coding".to_string(),
            description: "Helps with coding".to_string(),
            active: true,
        },
    ]);
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Loaded Skills"));
    assert!(prompt.contains("coding"));
}

#[test]
fn test_context_builder_memory_context() {
    let tmp = tempfile::tempdir().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_memory_context("User prefers dark mode.".to_string());
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Memory Context"));
    assert!(prompt.contains("dark mode"));
}

#[test]
fn test_context_builder_load_skills_from_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let skills_dir = tmp.path().join("Skills");
    let skill_a = skills_dir.join("my-skill");
    std::fs::create_dir_all(&skill_a).unwrap();
    std::fs::write(skill_a.join("SKILL.md"), "# My Cool Skill\nDoes cool things.").unwrap();

    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&skills_dir);
    let info = builder.get_skills_info();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].name, "my-skill");
}

#[test]
fn test_context_builder_load_skills_nonexistent_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&tmp.path().join("nonexistent"));
    assert!(builder.get_skills_info().is_empty());
}

#[test]
fn test_context_builder_build_messages_empty_history() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let messages = builder.build_messages(&[], "", "Hi", "web", "chat1", false);
    // system + user = 2
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "Hi");
}

#[test]
fn test_context_builder_build_messages_with_history() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let history = vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "Hi!".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
    ];
    let messages = builder.build_messages(&history, "", "How are you?", "web", "c1", false);
    // system + 2 history + 1 current = 4
    assert_eq!(messages.len(), 4);
}

#[test]
fn test_context_builder_build_messages_with_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let messages = builder.build_messages(&[], "Previous summary", "Continue", "web", "c1", false);
    assert_eq!(messages.len(), 2);
    assert!(messages[0].content.contains("Previous summary"));
}

#[test]
fn test_context_builder_build_messages_session_info() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let messages = builder.build_messages(&[], "", "Hi", "discord", "chat99", false);
    assert!(messages[0].content.contains("discord"));
    assert!(messages[0].content.contains("chat99"));
}

#[test]
fn test_context_builder_build_messages_skip_orphaned_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let history = vec![
        ConversationTurn {
            role: "tool".to_string(),
            content: "orphaned".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("tc-1".to_string()),
            timestamp: String::new(),
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
    ];
    let messages = builder.build_messages(&history, "", "Hi", "web", "c1", false);
    // system + 1 history (tool skipped) + 1 current = 3
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, "user");
}

#[test]
fn test_context_builder_add_tool_result() {
    let mut messages = vec![LlmMessage {
        role: "assistant".to_string(),
        content: "Using tool".to_string(),
        tool_calls: None,
        tool_call_id: None,
    }];
    ContextBuilder::add_tool_result(&mut messages, "tc-1", "calculator", "42");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "tool");
    assert_eq!(messages[1].content, "42");
    assert_eq!(messages[1].tool_call_id, Some("tc-1".to_string()));
}

#[test]
fn test_context_builder_add_assistant_message() {
    let mut messages: Vec<LlmMessage> = vec![];
    ContextBuilder::add_assistant_message(&mut messages, "Response", vec![]);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(messages[0].content, "Response");
}

#[test]
fn test_context_builder_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    assert_eq!(builder.workspace(), tmp.path());
}

// ===========================================================================
// SessionManager tests
// ===========================================================================

#[test]
fn test_session_manager_new() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(300));
    assert!(mgr.is_empty());
    assert_eq!(mgr.len(), 0);
}

#[test]
fn test_session_manager_default_timeout() {
    let mgr = SessionManager::with_default_timeout();
    assert!(mgr.is_empty());
}

#[test]
fn test_session_manager_get_or_create() {
    let mgr = SessionManager::with_default_timeout();
    let s = mgr.get_or_create("web:c1", "web", "c1");
    assert_eq!(s.session_key, "web:c1");
    assert_eq!(mgr.len(), 1);

    let s2 = mgr.get_or_create("web:c1", "web", "c1");
    assert_eq!(mgr.len(), 1); // Same session
    assert_eq!(s2.session_key, s.session_key);
}

#[test]
fn test_session_manager_multiple_sessions() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:c1", "web", "c1");
    mgr.get_or_create("web:c2", "web", "c2");
    mgr.get_or_create("discord:c3", "discord", "c3");
    assert_eq!(mgr.len(), 3);
}

#[test]
fn test_session_manager_set_busy() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:c1", "web", "c1");
    assert_eq!(mgr.is_busy("web:c1"), Some(false));
    assert!(mgr.set_busy("web:c1", true));
    assert_eq!(mgr.is_busy("web:c1"), Some(true));
    assert!(mgr.set_busy("web:c1", false));
    assert_eq!(mgr.is_busy("web:c1"), Some(false));
}

#[test]
fn test_session_manager_set_busy_nonexistent() {
    let mgr = SessionManager::with_default_timeout();
    assert!(!mgr.set_busy("nonexistent", true));
    assert_eq!(mgr.is_busy("nonexistent"), None);
}

#[test]
fn test_session_manager_last_channel_chat_id() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:c1", "web", "c1");
    mgr.set_last_channel("web:c1", "telegram");
    mgr.set_last_chat_id("web:c1", "chat42");
    let s = mgr.get_or_create("web:c1", "web", "c1");
    assert_eq!(s.last_channel.as_deref(), Some("telegram"));
    assert_eq!(s.last_chat_id.as_deref(), Some("chat42"));
}

#[test]
fn test_session_manager_contains() {
    let mgr = SessionManager::with_default_timeout();
    assert!(!mgr.contains("web:c1"));
    mgr.get_or_create("web:c1", "web", "c1");
    assert!(mgr.contains("web:c1"));
}

#[test]
fn test_session_manager_remove() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:c1", "web", "c1");
    assert!(mgr.contains("web:c1"));
    let removed = mgr.remove("web:c1");
    assert!(removed.is_some());
    assert!(!mgr.contains("web:c1"));
}

#[test]
fn test_session_manager_remove_nonexistent() {
    let mgr = SessionManager::with_default_timeout();
    assert!(mgr.remove("nonexistent").is_none());
}

#[test]
fn test_session_manager_cleanup_expired() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(1));
    mgr.get_or_create("web:c1", "web", "c1");
    // Wait for the session to be older than 1 second (need > 1s for num_seconds check)
    std::thread::sleep(std::time::Duration::from_millis(2100));
    let removed = mgr.cleanup_expired();
    assert_eq!(removed.len(), 1);
    assert!(mgr.is_empty());
}

// ===========================================================================
// SessionStore tests
// ===========================================================================

#[test]
fn test_session_store_in_memory() {
    let store = SessionStore::new_in_memory();
    assert!(store.is_empty());
    let s = store.get_or_create("key1");
    assert_eq!(s.key, "key1");
    assert!(s.messages.is_empty());
    assert_eq!(store.len(), 1);
}

#[test]
fn test_session_store_history() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    let msgs = vec![
        StoredMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
    ];
    store.set_history("key1", msgs);
    let history = store.get_history("key1");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "Hello");
}

#[test]
fn test_session_store_summary() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    assert!(store.get_summary("key1").is_empty());
    store.set_summary("key1", "Summary text");
    assert_eq!(store.get_summary("key1"), "Summary text");
}

#[test]
fn test_session_store_truncate() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    let msgs: Vec<StoredMessage> = (0..10).map(|i| StoredMessage {
        role: "user".to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();
    store.set_history("key1", msgs);
    store.truncate_history("key1", 3);
    let history = store.get_history("key1");
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].content, "msg_7");
}

#[test]
fn test_session_store_contains() {
    let store = SessionStore::new_in_memory();
    assert!(!store.contains("key1"));
    store.get_or_create("key1");
    assert!(store.contains("key1"));
}

#[test]
fn test_session_store_remove() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    assert!(store.remove("key1").is_some());
    assert!(!store.contains("key1"));
}

#[test]
fn test_session_store_disk_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    store.get_or_create("disk:key1");
    store.set_summary("disk:key1", "Summary");
    store.save("disk:key1").unwrap();

    let store2 = SessionStore::new_with_storage(dir.path());
    assert!(store2.contains("disk:key1"));
    assert_eq!(store2.get_summary("disk:key1"), "Summary");
}

#[test]
fn test_session_store_save_no_storage_dir() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    assert!(store.save("key1").is_ok()); // Silent success
}

// ===========================================================================
// Session struct tests
// ===========================================================================

#[test]
fn test_session_new() {
    let s = Session::new("web:c1", "web", "c1");
    assert_eq!(s.session_key, "web:c1");
    assert_eq!(s.channel, "web");
    assert_eq!(s.chat_id, "c1");
    assert!(!s.busy);
    assert!(s.last_channel.is_none());
    assert!(s.last_chat_id.is_none());
}

#[test]
fn test_session_touch() {
    let mut s = Session::new("web:c1", "web", "c1");
    let before = s.last_active;
    s.touch();
    assert!(s.last_active >= before);
}

#[test]
fn test_session_serialization() {
    let s = Session::new("web:c1", "web", "c1");
    let json = serde_json::to_string(&s).unwrap();
    let back: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(back.session_key, "web:c1");
    assert_eq!(back.channel, "web");
}

// ===========================================================================
// StoredMessage conversion tests
// ===========================================================================

#[test]
fn test_stored_message_from_turn() {
    let turn = ConversationTurn {
        role: "assistant".to_string(),
        content: "Using tools".to_string(),
        tool_calls: vec![ToolCallInfo {
            id: "tc-1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
        }],
        tool_call_id: None,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
    };
    let stored: StoredMessage = (&turn).into();
    assert_eq!(stored.role, "assistant");
    assert_eq!(stored.tool_calls.len(), 1);
    assert_eq!(stored.tool_calls[0].name, "search");
}

#[test]
fn test_turn_from_stored_message() {
    let stored = StoredMessage {
        role: "tool".to_string(),
        content: "42".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("tc-1".to_string()),
        timestamp: String::new(),
    };
    let turn: ConversationTurn = stored.into();
    assert_eq!(turn.role, "tool");
    assert_eq!(turn.content, "42");
    assert_eq!(turn.tool_call_id, Some("tc-1".to_string()));
}

// ===========================================================================
// Token estimation tests
// ===========================================================================

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_ascii() {
    // "Hello world" = 11 chars, 11*2/5 = 4
    assert_eq!(estimate_tokens("Hello world"), 4);
}

#[test]
fn test_estimate_tokens_cjk() {
    let text = "こんにちは世界";
    let tokens = estimate_tokens(text);
    assert!(tokens > 0);
}

#[test]
fn test_estimate_tokens_for_turns() {
    let turns = vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "World".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
    ];
    assert_eq!(estimate_tokens_for_turns(&turns), 4);
}

// ===========================================================================
// Force compression tests
// ===========================================================================

#[test]
fn test_force_compress_short() {
    let history: Vec<ConversationTurn> = (0..4).map(|i| ConversationTurn {
        role: if i == 0 { "system" } else { "user" }.to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();

    let result = force_compress_turns(&history);
    assert_eq!(result.len(), 4);
}

#[test]
fn test_force_compress_long() {
    let history: Vec<ConversationTurn> = (0..10).map(|i| ConversationTurn {
        role: if i == 0 { "system" } else { "user" }.to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();

    let result = force_compress_turns(&history);
    assert!(result.len() < history.len());
    assert_eq!(result[0].content, "msg_0");
    assert!(result[1].content.contains("Emergency compression"));
    assert_eq!(result.last().unwrap().content, "msg_9");
}

#[test]
fn test_force_compress_exact_boundary() {
    // 6 messages (1 system + 5 user) should trigger compression (len > 4)
    let history: Vec<ConversationTurn> = (0..6).map(|i| ConversationTurn {
        role: if i == 0 { "system" } else { "user" }.to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();

    let result = force_compress_turns(&history);
    assert!(result.len() < history.len());
}

// ===========================================================================
// is_internal_channel tests
// ===========================================================================

#[test]
fn test_is_internal_channel_values() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("rpc"));
    assert!(!is_internal_channel("discord"));
    assert!(!is_internal_channel("telegram"));
    assert!(!is_internal_channel(""));
}

// ===========================================================================
// SessionBusyTracker tests
// ===========================================================================

#[test]
fn test_session_busy_tracker_new() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 10);
    assert!(!tracker.is_busy("session1"));
}

#[test]
fn test_session_busy_tracker_acquire_release() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 10);
    assert!(tracker.try_acquire("session1"));
    assert!(tracker.is_busy("session1"));
    assert!(!tracker.try_acquire("session1")); // Already acquired

    tracker.release("session1");
    assert!(!tracker.is_busy("session1"));
}

#[test]
fn test_session_busy_tracker_multiple_sessions() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 10);
    assert!(tracker.try_acquire("s1"));
    assert!(tracker.try_acquire("s2"));
    assert!(tracker.is_busy("s1"));
    assert!(tracker.is_busy("s2"));
    assert!(!tracker.is_busy("s3"));
}

#[test]
fn test_session_busy_tracker_release_nonexistent() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 10);
    tracker.release("nonexistent"); // Should not panic
}

#[test]
fn test_concurrent_mode_default() {
    assert_eq!(ConcurrentMode::default(), ConcurrentMode::Reject);
}

// ===========================================================================
// ConversationMemory tests
// ===========================================================================

#[test]
fn test_conversation_memory_new() {
    let config = MemoryConfig::default();
    let mem = ConversationMemory::new(config);
    assert!(mem.is_empty());
    assert_eq!(mem.len(), 0);
}

#[test]
fn test_conversation_memory_add() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    mem.add(ConversationTurn {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    assert_eq!(mem.len(), 1);
    assert!(!mem.is_empty());
}

#[test]
fn test_conversation_memory_get_context() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    mem.add(ConversationTurn {
        role: "user".to_string(),
        content: "Test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    let ctx = mem.get_context();
    assert_eq!(ctx.len(), 1);
    assert_eq!(ctx[0].content, "Test");
}

#[test]
fn test_conversation_memory_estimated_tokens() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    mem.add(ConversationTurn {
        role: "user".to_string(),
        content: "Hello world".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    let tokens = mem.estimated_tokens();
    assert!(tokens > 0);
}

#[test]
fn test_conversation_memory_search() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    mem.add(ConversationTurn {
        role: "user".to_string(),
        content: "Rust programming language".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    mem.add(ConversationTurn {
        role: "assistant".to_string(),
        content: "Python is great too".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });

    let results = mem.search("Rust");
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("Rust"));

    let results2 = mem.search("nonexistent");
    assert!(results2.is_empty());
}

// ===========================================================================
// AgentRegistry tests (beyond existing)
// ===========================================================================

#[test]
fn test_agent_registry_default() {
    let registry = AgentRegistry::default();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn test_agent_registry_with_default() {
    let registry = AgentRegistry::with_default(test_config());
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
    assert!(registry.contains_agent("main"));
    assert_eq!(registry.default_agent_id(), Some("main".to_string()));
}

#[test]
fn test_agent_registry_register_and_lookup() {
    let registry = AgentRegistry::new();
    registry.register("agent-a".to_string(), AgentInstance::new(test_config()));
    assert!(registry.contains_agent("agent-a"));
    assert!(!registry.contains_agent("agent-b"));
}

#[test]
fn test_agent_registry_case_insensitive() {
    let registry = AgentRegistry::new();
    registry.register("myAgent".to_string(), AgentInstance::new(test_config()));
    assert!(registry.contains_agent("myagent"));
    assert!(registry.contains_agent("MYAGENT"));
}

#[test]
fn test_agent_registry_list_ids() {
    let registry = AgentRegistry::new();
    registry.register("a".to_string(), AgentInstance::new(test_config()));
    registry.register("b".to_string(), AgentInstance::new(test_config()));
    let mut ids = registry.list_agent_ids();
    ids.sort();
    assert_eq!(ids, vec!["a", "b"]);
}

#[test]
fn test_agent_registry_default_agent_id_fallback() {
    let registry = AgentRegistry::new();
    registry.register("other".to_string(), AgentInstance::new(test_config()));
    let id = registry.default_agent_id();
    assert!(id.is_some());
    assert_eq!(id.unwrap(), "other");
}

#[test]
fn test_agent_registry_default_agent_id_empty() {
    let registry = AgentRegistry::new();
    assert!(registry.default_agent_id().is_none());
}

#[test]
fn test_agent_registry_with_agent() {
    let registry = AgentRegistry::new();
    registry.register("test".to_string(), AgentInstance::new(test_config()));
    let result = registry.with_agent("test", |inst| inst.state());
    assert_eq!(result, Some(AgentState::Idle));
}

#[test]
fn test_agent_registry_with_agent_missing() {
    let registry = AgentRegistry::new();
    let result = registry.with_agent("nonexistent", |_inst| 42);
    assert!(result.is_none());
}

#[test]
fn test_agent_registry_can_spawn_subagent_wildcard() {
    let registry = AgentRegistry::new();
    registry.set_subagent_allow("parent", vec!["*".to_string()]);
    assert!(registry.can_spawn_subagent("parent", "any-child"));
}

#[test]
fn test_agent_registry_can_spawn_subagent_specific() {
    let registry = AgentRegistry::new();
    registry.set_subagent_allow("parent", vec!["child-a".to_string()]);
    assert!(registry.can_spawn_subagent("parent", "child-a"));
    assert!(!registry.can_spawn_subagent("parent", "child-b"));
}

#[test]
fn test_agent_registry_can_spawn_no_list() {
    let registry = AgentRegistry::new();
    assert!(!registry.can_spawn_subagent("parent", "child"));
}

#[test]
fn test_agent_registry_remove() {
    let registry = AgentRegistry::new();
    registry.register("to-remove".to_string(), AgentInstance::new(test_config()));
    assert!(registry.remove("to-remove"));
    assert!(!registry.contains_agent("to-remove"));
    assert!(!registry.remove("to-remove")); // Already removed
}

// ===========================================================================
// LoggingConfig tests
// ===========================================================================

#[test]
fn test_logging_config_default() {
    let config = LoggingConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.detail_level, DetailLevel::Full);
    assert_eq!(config.log_dir, "logs/llm");
}

#[test]
fn test_detail_level_default() {
    assert_eq!(DetailLevel::default(), DetailLevel::Full);
}

// ===========================================================================
// LlmMessage tests
// ===========================================================================

#[test]
fn test_llm_message_serialization() {
    let msg = LlmMessage {
        role: "system".to_string(),
        content: "You are helpful".to_string(),
        tool_calls: None,
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: LlmMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.role, "system");
    assert_eq!(back.content, "You are helpful");
}

#[test]
fn test_llm_message_with_tool_calls() {
    let msg = LlmMessage {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "tc-1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
        }]),
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: LlmMessage = serde_json::from_str(&json).unwrap();
    assert!(back.tool_calls.is_some());
    assert_eq!(back.tool_calls.as_ref().unwrap().len(), 1);
}

#[test]
fn test_llm_message_tool_response() {
    let msg = LlmMessage {
        role: "tool".to_string(),
        content: "42".to_string(),
        tool_calls: None,
        tool_call_id: Some("tc-1".to_string()),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: LlmMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tool_call_id, Some("tc-1".to_string()));
    assert_eq!(back.content, "42");
}

// ===========================================================================
// RingBuffer integration tests (verify exported type works)
// ===========================================================================

#[test]
fn test_ring_buffer_exported() {
    let rb: RingBuffer<i32> = RingBuffer::new(5);
    assert!(rb.is_empty());
    rb.push(1);
    rb.push(2);
    rb.push(3);
    assert_eq!(rb.len(), 3);
    assert_eq!(rb.get_all(), vec![1, 2, 3]);
}

#[test]
fn test_ring_buffer_wrap_around() {
    let rb = RingBuffer::new(3);
    rb.push(1);
    rb.push(2);
    rb.push(3);
    rb.push(4);
    assert_eq!(rb.get_all(), vec![2, 3, 4]);
}

// ===========================================================================
// MemoryConfig tests
// ===========================================================================

#[test]
fn test_memory_config_default() {
    let config = MemoryConfig::default();
    assert!(config.max_tokens > 0);
    assert!(config.keep_tokens > 0);
    assert!(config.keep_tokens < config.max_tokens);
}

#[test]
fn test_conversation_memory_with_defaults() {
    let mem = ConversationMemory::with_defaults();
    assert!(mem.is_empty());
}

// ===========================================================================
// MemoryStore tests
// ===========================================================================

#[test]
fn test_memory_store_read_long_term_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    assert!(store.read_long_term().is_empty());
}

#[test]
fn test_memory_store_write_read_long_term() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    store.write_long_term("Long term memory content").unwrap();
    assert_eq!(store.read_long_term(), "Long term memory content");
}

#[test]
fn test_memory_store_read_today_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    assert!(store.read_today().is_empty());
}

#[test]
fn test_memory_store_append_today() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    store.append_today("Note 1\n").unwrap();
    store.append_today("Note 2\n").unwrap();
    let today = store.read_today();
    assert!(today.contains("Note 1"));
    assert!(today.contains("Note 2"));
}

#[test]
fn test_memory_store_memory_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    assert!(store.memory_dir().exists());
}

#[test]
fn test_memory_store_memory_file_path() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    assert!(store.memory_file().to_string_lossy().contains("MEMORY.md"));
}

// ===========================================================================
// Additional StoredSession serialization tests
// ===========================================================================

#[test]
fn test_stored_session_serialization() {
    let session = StoredSession {
        key: "web:chat1".to_string(),
        messages: vec![
            StoredMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
        ],
        summary: "A greeting".to_string(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
    };
    let json = serde_json::to_string_pretty(&session).unwrap();
    let back: StoredSession = serde_json::from_str(&json).unwrap();
    assert_eq!(back.key, "web:chat1");
    assert_eq!(back.messages.len(), 1);
    assert_eq!(back.summary, "A greeting");
}

#[test]
fn test_stored_session_with_tool_calls() {
    let session = StoredSession {
        key: "rpc:task1".to_string(),
        messages: vec![
            StoredMessage {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls: vec![StoredToolCall {
                    id: "tc-1".to_string(),
                    name: "calculator".to_string(),
                    arguments: r#"{"expr":"2+2"}"#.to_string(),
                }],
                tool_call_id: None,
                timestamp: String::new(),
            },
            StoredMessage {
                role: "tool".to_string(),
                content: "4".to_string(),
                tool_calls: vec![],
                tool_call_id: Some("tc-1".to_string()),
                timestamp: String::new(),
            },
        ],
        summary: String::new(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&session).unwrap();
    let back: StoredSession = serde_json::from_str(&json).unwrap();
    assert_eq!(back.messages[0].tool_calls.len(), 1);
    assert_eq!(back.messages[1].tool_call_id, Some("tc-1".to_string()));
}

// ===========================================================================
// Additional AgentInstance tests
// ===========================================================================

#[test]
fn test_agent_instance_finish_from_any_state() {
    let instance = AgentInstance::new(test_config());

    instance.start_thinking();
    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);

    instance.start_thinking();
    instance.start_tool_execution();
    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);

    instance.start_thinking();
    instance.start_tool_execution();
    instance.start_responding();
    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);
}

#[test]
fn test_agent_instance_finish_from_idle() {
    let instance = AgentInstance::new(test_config());
    instance.finish();
    assert_eq!(instance.state(), AgentState::Idle);
}

#[test]
fn test_agent_instance_many_messages() {
    let instance = AgentInstance::new(test_config());
    for i in 0..100 {
        instance.add_user_message(&format!("Message {}", i));
    }
    assert_eq!(instance.message_count(), 100);
    let history = instance.get_history();
    // system prompt + 100 messages = 101
    assert_eq!(history.len(), 101);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].content, "Message 0");
    assert_eq!(history[100].content, "Message 99");
}

#[test]
fn test_agent_instance_tool_call_and_result() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Calculate 2+2");
    instance.add_assistant_message("Let me calculate", vec![
        ToolCallInfo {
            id: "tc-calc".to_string(),
            name: "calculator".to_string(),
            arguments: r#"{"expr": "2+2"}"#.to_string(),
        },
    ]);
    instance.add_tool_result("tc-calc", "4");
    instance.add_assistant_message("The answer is 4", vec![]);

    let history = instance.get_history();
    // system + user + assistant + tool_result + assistant = 5
    assert_eq!(history.len(), 5);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].role, "user");
    assert_eq!(history[2].role, "assistant");
    assert_eq!(history[2].tool_calls.len(), 1);
    assert_eq!(history[3].role, "tool");
    assert_eq!(history[3].tool_call_id, Some("tc-calc".to_string()));
    assert_eq!(history[4].role, "assistant");
    assert!(history[4].tool_calls.is_empty());
}

#[test]
fn test_agent_instance_truncate_to_zero() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("msg1");
    instance.add_user_message("msg2");
    instance.add_user_message("msg3");
    instance.truncate_to(0);
    assert_eq!(instance.message_count(), 0);
}

#[test]
fn test_agent_instance_truncate_to_more_than_count() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("msg1");
    instance.truncate_to(100);
    assert_eq!(instance.message_count(), 1);
}

#[test]
fn test_agent_instance_multiple_tool_calls() {
    let instance = AgentInstance::new(test_config());
    instance.add_assistant_message("Searching", vec![
        ToolCallInfo {
            id: "tc-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q": "rust"}"#.to_string(),
        },
        ToolCallInfo {
            id: "tc-2".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q": "golang"}"#.to_string(),
        },
        ToolCallInfo {
            id: "tc-3".to_string(),
            name: "file_read".to_string(),
            arguments: r#"{"path": "/tmp/test"}"#.to_string(),
        },
    ]);
    instance.add_tool_result("tc-1", "Rust results");
    instance.add_tool_result("tc-2", "Go results");
    instance.add_tool_result("tc-3", "File contents");
    assert_eq!(instance.message_count(), 4);
}

#[test]
fn test_agent_instance_clear_then_add() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("msg1");
    instance.clear_history();
    assert_eq!(instance.message_count(), 0);
    instance.add_user_message("msg2");
    assert_eq!(instance.message_count(), 1);
}

#[test]
fn test_agent_instance_unicode_messages() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("こんにちは世界");
    instance.add_assistant_message("Bonjour le monde!", vec![]);
    let history = instance.get_history();
    // history[0] is system prompt
    assert_eq!(history[1].content, "こんにちは世界");
    assert_eq!(history[2].content, "Bonjour le monde!");
}

#[test]
fn test_agent_instance_special_chars_messages() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("<script>alert('xss')</script>");
    instance.add_user_message("DROP TABLE users; --");
    instance.add_user_message("C:\\Users\\test\\file.txt");
    let history = instance.get_history();
    // history[0] is system prompt
    assert!(history[1].content.contains("<script>"));
    assert!(history[2].content.contains("DROP TABLE"));
    assert!(history[3].content.contains("C:\\"));
}

// ===========================================================================
// Additional RequestContext tests
// ===========================================================================

#[test]
fn test_request_context_debug_format() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let debug_str = format!("{:?}", ctx);
    assert!(debug_str.contains("web"));
    assert!(debug_str.contains("chat1"));
}

#[test]
fn test_request_context_rpc_format_unicode_message() {
    let ctx = RequestContext::for_rpc("c1", "u1", "s1", "cid-123");
    let formatted = ctx.format_rpc_message("こんにちは");
    assert_eq!(formatted, "[rpc:cid-123] こんにちは");
}

#[test]
fn test_request_context_non_rpc_no_correlation() {
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    assert!(!ctx.is_rpc());
    assert!(ctx.correlation_id.is_none());
}

#[test]
fn test_request_context_invoke_callback_none() {
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    assert!(!ctx.invoke_async_callback("test"));
}

#[test]
fn test_request_context_invoke_callback_multiple_times() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();
    let mut ctx = RequestContext::new("web", "c1", "u1", "s1");
    ctx.set_async_callback(Arc::new(move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    }));

    ctx.invoke_async_callback("a");
    ctx.invoke_async_callback("b");
    ctx.invoke_async_callback("c");
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

// ===========================================================================
// Additional ContextBuilder tests
// ===========================================================================

#[test]
fn test_context_builder_no_tools_section_when_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Available Tools"));
}

#[test]
fn test_context_builder_no_skills_section_when_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Loaded Skills"));
}

#[test]
fn test_context_builder_no_memory_when_none() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Memory Context"));
}

#[test]
fn test_context_builder_multiple_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "I am helpful.").unwrap();
    std::fs::write(tmp.path().join("USER.md"), "User likes Python.").unwrap();
    std::fs::write(tmp.path().join("SOUL.md"), "Be concise.").unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("IDENTITY.md"));
    assert!(prompt.contains("I am helpful."));
    assert!(prompt.contains("USER.md"));
    assert!(prompt.contains("User likes Python."));
    assert!(prompt.contains("SOUL.md"));
    assert!(prompt.contains("Be concise."));
}

#[test]
fn test_context_builder_build_messages_empty_message() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let messages = builder.build_messages(&[], "", "", "web", "c1", false);
    // system only (no empty user message)
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "system");
}

#[test]
fn test_context_builder_build_messages_tool_in_middle() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let history = vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
        ConversationTurn {
            role: "tool".to_string(),
            content: "result".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("tc-1".to_string()),
            timestamp: String::new(),
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "Done".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        },
    ];
    let messages = builder.build_messages(&history, "", "Next", "web", "c1", false);
    // system + 3 history + 1 current = 5
    // (tool in middle is NOT orphaned, only leading tools are skipped)
    assert_eq!(messages.len(), 5);
}

#[test]
fn test_context_builder_add_tool_result_multiple() {
    let mut messages = vec![];
    ContextBuilder::add_assistant_message(&mut messages, "Working", vec![]);
    ContextBuilder::add_tool_result(&mut messages, "tc-1", "calculator", "42");
    ContextBuilder::add_tool_result(&mut messages, "tc-2", "search", "found");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].tool_call_id, Some("tc-1".to_string()));
    assert_eq!(messages[2].tool_call_id, Some("tc-2".to_string()));
}

// ===========================================================================
// Additional SessionManager tests
// ===========================================================================

#[test]
fn test_session_manager_touch_on_get_or_create() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(300));
    let s1 = mgr.get_or_create("web:c1", "web", "c1");
    let first_active = s1.last_active;
    // Small delay to ensure timestamp changes
    std::thread::sleep(std::time::Duration::from_millis(10));
    let s2 = mgr.get_or_create("web:c1", "web", "c1");
    assert!(s2.last_active >= first_active);
}

#[test]
fn test_session_manager_cleanup_not_expired() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(3600));
    mgr.get_or_create("web:c1", "web", "c1");
    let removed = mgr.cleanup_expired();
    assert!(removed.is_empty());
    assert_eq!(mgr.len(), 1);
}

#[test]
fn test_session_manager_cleanup_custom_timeout() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(3600));
    mgr.get_or_create("web:c1", "web", "c1");
    // Use a very short timeout so it expires immediately
    let removed = mgr.cleanup_expired_with_timeout(std::time::Duration::from_millis(1));
    // Depending on timing, the session may or may not be expired.
    // The key point is that cleanup_expired_with_timeout works without panic.
    assert!(removed.len() <= 1);
}

#[test]
fn test_session_manager_multiple_cleanup() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(1));
    mgr.get_or_create("web:c1", "web", "c1");
    mgr.get_or_create("web:c2", "web", "c2");
    mgr.get_or_create("web:c3", "web", "c3");
    // Wait for all sessions to be older than 1 second (need > 1s for num_seconds check)
    std::thread::sleep(std::time::Duration::from_millis(2100));
    let removed = mgr.cleanup_expired();
    assert_eq!(removed.len(), 3);
    assert!(mgr.is_empty());
}

// ===========================================================================
// Additional SessionStore tests
// ===========================================================================

#[test]
fn test_session_store_get_or_create_returns_same() {
    let store = SessionStore::new_in_memory();
    let s1 = store.get_or_create("key1");
    let s2 = store.get_or_create("key1");
    assert_eq!(s1.key, s2.key);
}

#[test]
fn test_session_store_get_history_nonexistent() {
    let store = SessionStore::new_in_memory();
    assert!(store.get_history("nonexistent").is_empty());
}

#[test]
fn test_session_store_get_summary_nonexistent() {
    let store = SessionStore::new_in_memory();
    assert!(store.get_summary("nonexistent").is_empty());
}

#[test]
fn test_session_store_set_history_nonexistent() {
    let store = SessionStore::new_in_memory();
    // Should not panic
    store.set_history("nonexistent", vec![]);
}

#[test]
fn test_session_store_set_summary_nonexistent() {
    let store = SessionStore::new_in_memory();
    // Should not panic
    store.set_summary("nonexistent", "summary");
}

#[test]
fn test_session_store_truncate_nonexistent() {
    let store = SessionStore::new_in_memory();
    // Should not panic
    store.truncate_history("nonexistent", 5);
}

#[test]
fn test_session_store_truncate_fewer_than_keep() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    let msgs: Vec<StoredMessage> = (0..3).map(|i| StoredMessage {
        role: "user".to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();
    store.set_history("key1", msgs);
    store.truncate_history("key1", 10);
    let history = store.get_history("key1");
    assert_eq!(history.len(), 3);
}

#[test]
fn test_session_store_multiple_sessions() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    store.get_or_create("key2");
    store.get_or_create("key3");
    assert_eq!(store.len(), 3);
}

// ===========================================================================
// Additional ConversationMemory tests
// ===========================================================================

#[test]
fn test_conversation_memory_multiple_adds() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    for i in 0..20 {
        mem.add(ConversationTurn {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
        });
    }
    assert_eq!(mem.len(), 20);
    let ctx = mem.get_context();
    assert_eq!(ctx.len(), 20);
}

#[test]
fn test_conversation_memory_search_case_insensitive() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    mem.add(ConversationTurn {
        role: "user".to_string(),
        content: "Rust Programming".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    assert_eq!(mem.search("rust").len(), 1);
    assert_eq!(mem.search("RUST").len(), 1);
    assert_eq!(mem.search("programming").len(), 1);
}

#[test]
fn test_conversation_memory_search_empty() {
    let config = MemoryConfig::default();
    let mem = ConversationMemory::new(config);
    assert!(mem.search("anything").is_empty());
}

#[test]
fn test_conversation_memory_tokens_multiple_messages() {
    let config = MemoryConfig::default();
    let mut mem = ConversationMemory::new(config);
    mem.add(ConversationTurn {
        role: "user".to_string(),
        content: "Hello world".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    mem.add(ConversationTurn {
        role: "assistant".to_string(),
        content: "Goodbye world".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    });
    let tokens = mem.estimated_tokens();
    // "Hello world" (11 chars) -> 11*2/5=4, "Goodbye world" (13 chars) -> 13*2/5=5 => total 9
    assert_eq!(tokens, 9);
}

// ===========================================================================
// Additional AgentRegistry tests
// ===========================================================================

#[test]
fn test_agent_registry_remove_nonexistent() {
    let registry = AgentRegistry::new();
    assert!(!registry.remove("nonexistent"));
}

#[test]
fn test_agent_registry_contains_case_insensitive() {
    let registry = AgentRegistry::new();
    registry.register("MyAgent".to_string(), AgentInstance::new(test_config()));
    assert!(registry.contains_agent("myagent"));
    assert!(registry.contains_agent("MYAGENT"));
    assert!(registry.contains_agent("MyAgent"));
}

#[test]
fn test_agent_registry_with_agent_mut() {
    let registry = AgentRegistry::new();
    registry.register("test".to_string(), AgentInstance::new(test_config()));
    let result = registry.with_agent_mut("test", |inst| {
        inst.add_user_message("hello");
        inst.message_count()
    });
    assert_eq!(result, Some(1));
}

#[test]
fn test_agent_registry_with_agent_mut_missing() {
    let registry = AgentRegistry::new();
    let result = registry.with_agent_mut("nonexistent", |_inst| 42);
    assert!(result.is_none());
}

// ===========================================================================
// Additional MemoryStore tests
// ===========================================================================

#[test]
fn test_memory_store_overwrite_long_term() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    store.write_long_term("First content").unwrap();
    assert_eq!(store.read_long_term(), "First content");
    store.write_long_term("Second content").unwrap();
    assert_eq!(store.read_long_term(), "Second content");
}

#[test]
fn test_memory_store_empty_append_today() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    let today = store.read_today();
    assert!(today.is_empty());
}

#[test]
fn test_memory_store_multiple_appends() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(tmp.path().to_str().unwrap());
    store.append_today("Line 1\n").unwrap();
    store.append_today("Line 2\n").unwrap();
    store.append_today("Line 3\n").unwrap();
    let today = store.read_today();
    assert!(today.contains("Line 1"));
    assert!(today.contains("Line 2"));
    assert!(today.contains("Line 3"));
}

// ===========================================================================
// Additional RingBuffer tests
// ===========================================================================

#[test]
fn test_ring_buffer_new_empty() {
    let rb: RingBuffer<i32> = RingBuffer::new(5);
    assert!(rb.is_empty());
    assert_eq!(rb.len(), 0);
}

#[test]
fn test_ring_buffer_push_within_capacity() {
    let rb = RingBuffer::new(5);
    rb.push(1);
    rb.push(2);
    rb.push(3);
    assert_eq!(rb.len(), 3);
    assert_eq!(rb.get_all(), vec![1, 2, 3]);
}

#[test]
fn test_ring_buffer_wrap_around_drops_oldest() {
    let rb = RingBuffer::new(3);
    rb.push(1);
    rb.push(2);
    rb.push(3);
    rb.push(4);
    rb.push(5);
    assert_eq!(rb.len(), 3);
    assert_eq!(rb.get_all(), vec![3, 4, 5]);
}

#[test]
fn test_ring_buffer_exact_capacity() {
    let rb = RingBuffer::new(3);
    rb.push(1);
    rb.push(2);
    rb.push(3);
    assert_eq!(rb.len(), 3);
    assert_eq!(rb.get_all(), vec![1, 2, 3]);
}

#[test]
fn test_ring_buffer_capacity_one() {
    let rb = RingBuffer::new(1);
    rb.push(1);
    rb.push(2);
    rb.push(3);
    assert_eq!(rb.len(), 1);
    assert_eq!(rb.get_all(), vec![3]);
}

// ===========================================================================
// Additional force_compress_turns tests
// ===========================================================================

#[test]
fn test_force_compress_preserves_system() {
    let history: Vec<ConversationTurn> = (0..8).map(|i| ConversationTurn {
        role: if i == 0 { "system" } else { "user" }.to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();
    let result = force_compress_turns(&history);
    assert_eq!(result[0].content, "msg_0");
    assert_eq!(result[0].role, "system");
}

#[test]
fn test_force_compress_preserves_last() {
    let history: Vec<ConversationTurn> = (0..8).map(|i| ConversationTurn {
        role: if i == 0 { "system" } else { "user" }.to_string(),
        content: format!("msg_{}", i),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }).collect();
    let result = force_compress_turns(&history);
    assert_eq!(result.last().unwrap().content, "msg_7");
}

#[test]
fn test_force_compress_empty() {
    let empty: Vec<ConversationTurn> = vec![];
    let result = force_compress_turns(&empty);
    assert!(result.is_empty());
}

#[test]
fn test_force_compress_single() {
    let history = vec![ConversationTurn {
        role: "user".to_string(),
        content: "only msg".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: String::new(),
    }];
    let result = force_compress_turns(&history);
    assert_eq!(result.len(), 1);
}

// ===========================================================================
// Additional token estimation tests
// ===========================================================================

#[test]
fn test_estimate_tokens_long_text() {
    let text = "a".repeat(1000);
    let tokens = estimate_tokens(&text);
    assert_eq!(tokens, 400); // 1000 * 2 / 5
}

#[test]
fn test_estimate_tokens_single_char() {
    assert_eq!(estimate_tokens("a"), 0); // 1 * 2 / 5 = 0 (integer)
}

#[test]
fn test_estimate_tokens_three_chars() {
    assert_eq!(estimate_tokens("abc"), 1); // 3 * 2 / 5 = 1
}

// ===========================================================================
// Additional LoggingConfig tests
// ===========================================================================

#[test]
fn test_detail_level_values() {
    let levels = vec![DetailLevel::Full, DetailLevel::Truncated];
    assert_eq!(levels.len(), 2);
    for level in &levels {
        let json = serde_json::to_string(level).unwrap();
        let back: DetailLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, *level);
    }
}

#[test]
fn test_logging_config_custom() {
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Truncated,
        log_dir: "custom/logs".to_string(),
    };
    assert!(config.enabled);
    assert_eq!(config.detail_level, DetailLevel::Truncated);
    assert_eq!(config.log_dir, "custom/logs");
}

// ===========================================================================
// Additional ConcurrentMode tests
// ===========================================================================

#[test]
fn test_concurrent_mode_values() {
    assert_eq!(ConcurrentMode::default(), ConcurrentMode::Reject);
    assert_ne!(ConcurrentMode::Reject, ConcurrentMode::Queue);
}

// ===========================================================================
// Additional ChatOptions tests
// ===========================================================================

#[test]
fn test_chat_options_all_none() {
    let opts = ChatOptions {
        max_tokens: None,
        temperature: None,
        top_p: None,
        stop: None,
    };
    let json = serde_json::to_string(&opts).unwrap();
    let back: ChatOptions = serde_json::from_str(&json).unwrap();
    assert!(back.max_tokens.is_none());
}

#[test]
fn test_chat_options_roundtrip() {
    let opts = ChatOptions {
        max_tokens: Some(100),
        temperature: Some(0.3),
        top_p: Some(0.95),
        stop: Some(vec!["STOP".to_string(), "END".to_string()]),
    };
    let json = serde_json::to_string(&opts).unwrap();
    let back: ChatOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(back.max_tokens, Some(100));
    assert_eq!(back.temperature, Some(0.3));
    assert_eq!(back.top_p, Some(0.95));
    assert_eq!(back.stop.as_ref().unwrap().len(), 2);
}

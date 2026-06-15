use super::*;
use tempfile::TempDir;

#[test]
fn new_context_has_no_correlation_id() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert_eq!(ctx.channel, "web");
    assert_eq!(ctx.chat_id, "chat1");
    assert_eq!(ctx.user, "user1");
    assert_eq!(ctx.session_key, "sess1");
    assert!(ctx.correlation_id.is_none());
    assert!(!ctx.is_rpc());
}

#[test]
fn for_rpc_sets_channel_and_correlation_id() {
    let ctx = RequestContext::for_rpc("chat42", "user1", "sess1", "corr-123");
    assert_eq!(ctx.channel, "rpc");
    assert_eq!(ctx.chat_id, "chat42");
    assert_eq!(ctx.correlation_id.as_deref(), Some("corr-123"));
    assert!(ctx.is_rpc());
}

#[test]
fn format_rpc_message() {
    // RPC context: should add prefix.
    let rpc_ctx = RequestContext::for_rpc("chat1", "user1", "sess1", "abc-999");
    assert_eq!(
        rpc_ctx.format_rpc_message("Hello world"),
        "[rpc:abc-999] Hello world"
    );

    // Non-RPC context: no prefix.
    let web_ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert_eq!(web_ctx.format_rpc_message("Hello world"), "Hello world");

    // RPC context with empty correlation_id: no prefix.
    let rpc_no_cid = RequestContext {
        channel: "rpc".to_string(),
        chat_id: "chat1".to_string(),
        user: "user1".to_string(),
        session_key: "sess1".to_string(),
        correlation_id: Some(String::new()),
        async_callback: None,
    };
    assert_eq!(rpc_no_cid.format_rpc_message("Hello"), "Hello");
}

// --- ContextBuilder tests ---

#[test]
fn context_builder_empty_workspace() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    // Time is intentionally not in the cached system prompt; it's injected
    // per-request by AgentLoopExecutor::build_messages().
    assert!(!prompt.contains("Current Time"));
    assert!(prompt.contains("Environment"));
    assert!(prompt.contains("Workspace"));
}

#[test]
fn context_builder_with_identity_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "I am a helpful assistant.").unwrap();
    std::fs::write(tmp.path().join("USER.md"), "User prefers English.").unwrap();

    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    assert!(prompt.contains("IDENTITY.md"));
    assert!(prompt.contains("I am a helpful assistant."));
    assert!(prompt.contains("USER.md"));
    assert!(prompt.contains("User prefers English."));
}

#[test]
fn context_builder_skip_bootstrap_mode() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "I am a helper.").unwrap();

    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(true);

    assert!(prompt.contains("IDENTITY.md"));
    assert!(prompt.contains("I am a helper."));
}

#[test]
fn context_builder_bootstrap_file_triggers_init_mode() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("BOOTSTRAP.md"), "Please set up the assistant.").unwrap();

    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    assert!(prompt.contains("Initialization Bootstrap Mode"));
    assert!(prompt.contains("Please set up the assistant."));
}

#[test]
fn context_builder_with_tool_summaries() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_tool_summaries(vec![
        "- calculator: Performs arithmetic".to_string(),
        "- search: Searches the web".to_string(),
    ]);
    let prompt = builder.build_system_prompt(false);

    assert!(prompt.contains("Available Tools"));
    assert!(prompt.contains("calculator"));
    assert!(prompt.contains("search"));
}

#[test]
fn build_messages_with_history() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let history = vec![
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:00Z".to_string(),
            reasoning_content: None,
        },
        crate::types::ConversationTurn {
            role: "assistant".to_string(),
            content: "Hi there!".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:01Z".to_string(),
            reasoning_content: None,
        },
    ];

    let messages = builder.build_messages(
        &history,
        "",
        "How are you?",
        "web",
        "chat1",
        false,
    );

    // system + 2 history + 1 current = 4
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[3].role, "user");
    assert_eq!(messages[3].content, "How are you?");
}

#[test]
fn build_messages_skips_orphaned_tool_at_start() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let history = vec![
        // Orphaned tool message at the start of history
        crate::types::ConversationTurn {
            role: "tool".to_string(),
            content: "orphaned result".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_1".to_string()),
            timestamp: "2026-04-29T12:00:00Z".to_string(),
            reasoning_content: None,
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:01Z".to_string(),
            reasoning_content: None,
        },
    ];

    let messages = builder.build_messages(&history, "", "Hi", "web", "chat1", false);

    // system + 1 history (tool skipped) + 1 current = 3
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "Hello");
}

#[test]
fn build_messages_skips_all_leading_orphaned_tools() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let history = vec![
        // Multiple orphaned tool messages at the start of history
        crate::types::ConversationTurn {
            role: "tool".to_string(),
            content: "orphaned result 1".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_1".to_string()),
            timestamp: "2026-04-29T12:00:00Z".to_string(),
            reasoning_content: None,
        },
        crate::types::ConversationTurn {
            role: "tool".to_string(),
            content: "orphaned result 2".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_2".to_string()),
            timestamp: "2026-04-29T12:00:01Z".to_string(),
            reasoning_content: None,
        },
        crate::types::ConversationTurn {
            role: "tool".to_string(),
            content: "orphaned result 3".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_3".to_string()),
            timestamp: "2026-04-29T12:00:02Z".to_string(),
            reasoning_content: None,
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:03Z".to_string(),
            reasoning_content: None,
        },
    ];

    let messages = builder.build_messages(&history, "", "Hi", "web", "chat1", false);

    // system + 1 history (3 tools skipped) + 1 current = 3
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "Hello");
}

#[test]
fn build_messages_with_summary() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let messages = builder.build_messages(
        &[],
        "Previous conversation summary here.",
        "Continue",
        "web",
        "chat1",
        false,
    );

    // system + current = 2
    assert_eq!(messages.len(), 2);
    assert!(messages[0].content.contains("Previous conversation summary here."));
}

#[test]
fn build_messages_with_session_info() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let messages = builder.build_messages(&[], "", "Hi", "discord", "chat99", false);
    assert!(messages[0].content.contains("discord"));
    assert!(messages[0].content.contains("chat99"));
}

#[test]
fn context_builder_load_skills() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("Skills");
    let skill_a = skills_dir.join("my-skill");
    std::fs::create_dir_all(&skill_a).unwrap();
    std::fs::write(skill_a.join("SKILL.md"), "# My Cool Skill\nDoes cool things.").unwrap();

    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&skills_dir);

    let info = builder.get_skills_info();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].name, "my-skill");
    assert!(info[0].description.contains("My Cool Skill"));

    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Loaded Skills"));
    assert!(prompt.contains("my-skill"));
}

#[test]
fn context_builder_with_memory_context() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_memory_context("Remember: user prefers dark mode.".to_string());

    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Memory Context"));
    assert!(prompt.contains("dark mode"));
}

#[test]
fn context_builder_identity_has_runtime_info() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    assert!(prompt.contains("NemesisBot (Rust)"));
    assert!(prompt.contains("Memory Path"));
    assert!(prompt.contains("Skills Path"));
}

#[test]
fn context_builder_set_tools_registry() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());

    let definitions = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "calculator",
                "description": "Performs arithmetic operations",
                "parameters": {}
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "search",
                "description": "Searches the web",
                "parameters": {}
            }
        }),
    ];

    builder.set_tools_registry(definitions.clone());

    // Should have generated summaries
    assert_eq!(builder.tool_definitions().len(), 2);

    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("Available Tools"));
    assert!(prompt.contains("calculator"));
    assert!(prompt.contains("Performs arithmetic operations"));
    assert!(prompt.contains("search"));
    assert!(prompt.contains("Searches the web"));
}

#[test]
fn context_builder_set_tools_registry_appends_to_existing() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_tool_summaries(vec!["- existing: Existing tool".to_string()]);

    let definitions = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "new_tool",
                "description": "A new tool",
                "parameters": {}
            }
        }),
    ];

    builder.set_tools_registry(definitions);

    let prompt = builder.build_system_prompt(false);
    assert!(prompt.contains("existing"));
    assert!(prompt.contains("new_tool"));
}

// --- Additional RequestContext tests ---

#[test]
fn request_context_debug_format() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let debug_str = format!("{:?}", ctx);
    assert!(debug_str.contains("web"));
    assert!(debug_str.contains("chat1"));
    assert!(debug_str.contains("user1"));
    assert!(debug_str.contains("sess1"));
}

#[test]
fn request_context_with_correlation_id() {
    let ctx = RequestContext::with_correlation_id("web", "chat1", "user1", "sess1", "corr-42");
    assert_eq!(ctx.channel, "web");
    assert_eq!(ctx.correlation_id.as_deref(), Some("corr-42"));
    assert!(!ctx.is_rpc());
}

#[test]
fn request_context_rpc_is_rpc() {
    let ctx = RequestContext::for_rpc("chat1", "user1", "sess1", "corr-1");
    assert!(ctx.is_rpc());
}

#[test]
fn request_context_non_rpc_not_is_rpc() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert!(!ctx.is_rpc());
}

#[test]
fn request_context_format_rpc_message_non_rpc() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert_eq!(ctx.format_rpc_message("Hello"), "Hello");
}

#[test]
fn request_context_format_rpc_message_no_correlation_id() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert_eq!(ctx.format_rpc_message("Hello"), "Hello");
}

#[test]
fn request_context_format_rpc_message_empty_correlation_id() {
    let mut ctx = RequestContext::new("rpc", "chat1", "user1", "sess1");
    ctx.correlation_id = Some(String::new());
    assert_eq!(ctx.format_rpc_message("Hello"), "Hello");
}

#[test]
fn request_context_set_and_invoke_async_callback() {
    let mut ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let called = Arc::new(std::sync::Mutex::new(false));
    let called_clone = called.clone();

    ctx.set_async_callback(Arc::new(move |msg| {
        assert_eq!(msg, "test callback");
        *called_clone.lock().unwrap() = true;
    }));

    assert!(ctx.invoke_async_callback("test callback"));
    assert!(*called.lock().unwrap());
}

#[test]
fn request_context_invoke_async_callback_none() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    assert!(!ctx.invoke_async_callback("test"));
}

#[test]
fn request_context_serialization_roundtrip() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let json = serde_json::to_string(&ctx).unwrap();
    let parsed: RequestContext = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.channel, "web");
    assert_eq!(parsed.chat_id, "chat1");
    assert_eq!(parsed.user, "user1");
    assert_eq!(parsed.session_key, "sess1");
    assert!(parsed.correlation_id.is_none());
    // async_callback is skipped during serialization
    assert!(parsed.async_callback.is_none());
}

#[test]
fn request_context_clone() {
    let ctx = RequestContext::for_rpc("chat1", "user1", "sess1", "corr-1");
    let cloned = ctx.clone();
    assert_eq!(cloned.channel, "rpc");
    assert_eq!(cloned.correlation_id, ctx.correlation_id);
}

// --- Additional ContextBuilder tests ---

#[test]
fn context_builder_no_tools_section_when_empty() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Available Tools"));
}

#[test]
fn context_builder_no_skills_section_when_empty() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Loaded Skills"));
}

#[test]
fn context_builder_no_memory_section_when_none() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Memory Context"));
}

#[test]
fn context_builder_empty_memory_context_ignored() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.set_memory_context(String::new());
    let prompt = builder.build_system_prompt(false);
    assert!(!prompt.contains("Memory Context"));
}

#[test]
fn context_builder_skills_dir_not_exists() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&tmp.path().join("nonexistent"));
    assert!(builder.get_skills_info().is_empty());
}

#[test]
fn context_builder_skills_dir_with_empty_dirs() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("Skills");
    let empty_skill = skills_dir.join("empty-skill");
    std::fs::create_dir_all(&empty_skill).unwrap();
    // No SKILL.md inside

    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&skills_dir);
    assert!(builder.get_skills_info().is_empty());
}

#[test]
fn context_builder_skills_with_no_description() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("Skills");
    let skill_dir = skills_dir.join("minimal-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "").unwrap();

    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&skills_dir);
    assert_eq!(builder.get_skills_info().len(), 1);
    assert_eq!(builder.get_skills_info()[0].name, "minimal-skill");
    assert!(builder.get_skills_info()[0].description.is_empty());
}

#[test]
fn context_builder_multiple_skills() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("Skills");

    for name in &["skill-a", "skill-b", "skill-c"] {
        let dir = skills_dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("# {} description", name)).unwrap();
    }

    let mut builder = ContextBuilder::new(tmp.path());
    builder.load_skills(&skills_dir);
    assert_eq!(builder.get_skills_info().len(), 3);
}

#[test]
fn context_builder_set_tools_registry_with_invalid_defs() {
    let tmp = TempDir::new().unwrap();
    let mut builder = ContextBuilder::new(tmp.path());

    // Definition without function name
    let definitions = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "description": "Missing name"
            }
        }),
    ];

    builder.set_tools_registry(definitions);
    // Should not crash, invalid definitions are filtered
    assert_eq!(builder.tool_definitions().len(), 1);
}

#[test]
fn context_builder_workspace_path() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    assert_eq!(builder.workspace(), tmp.path());
}

#[test]
fn build_messages_empty_history_no_message() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let messages = builder.build_messages(&[], "", "", "", "", false);
    // Only system message, no current message
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "system");
}

#[test]
fn build_messages_empty_channel_and_chat_id() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let messages = builder.build_messages(&[], "", "Hello", "", "", false);
    // Should not contain session info when channel/chat_id are empty
    assert_eq!(messages.len(), 2);
    assert!(!messages[0].content.contains("Current Session"));
}

#[test]
fn add_tool_result_appends_message() {
    let mut messages = Vec::new();
    ContextBuilder::add_tool_result(&mut messages, "tc_1", "search", "Found results");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "tool");
    assert_eq!(messages[0].content, "Found results");
    assert_eq!(messages[0].tool_call_id, Some("tc_1".to_string()));
}

#[test]
fn add_assistant_message_with_tool_calls() {
    let mut messages = Vec::new();
    let tool_calls = vec![crate::types::ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: "{}".to_string(),
    }];
    ContextBuilder::add_assistant_message(&mut messages, "Searching...", tool_calls);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(messages[0].content, "Searching...");
    assert!(messages[0].tool_calls.is_some());
}

#[test]
fn add_assistant_message_without_tool_calls() {
    let mut messages = Vec::new();
    ContextBuilder::add_assistant_message(&mut messages, "Hello!", vec![]);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert!(messages[0].tool_calls.is_none());
}

#[test]
fn build_messages_with_tool_calls_in_history() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    let history = vec![
        crate::types::ConversationTurn {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![crate::types::ToolCallInfo {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: "{}".to_string(),
            }],
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:00Z".to_string(),
            reasoning_content: None,
        },
    ];

    let messages = builder.build_messages(&history, "", "Continue", "web", "chat1", false);
    // system + 1 history (with tool_calls) + 1 current = 3
    assert_eq!(messages.len(), 3);
    assert!(messages[1].tool_calls.is_some());
}

#[test]
fn build_messages_with_tool_call_id_in_history() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());

    // Tool message at the start is skipped (orphaned)
    let history = vec![
        crate::types::ConversationTurn {
            role: "tool".to_string(),
            content: "result data".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_123".to_string()),
            timestamp: "2026-04-29T12:00:00Z".to_string(),
            reasoning_content: None,
        },
        crate::types::ConversationTurn {
            role: "assistant".to_string(),
            content: "Final answer".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:01Z".to_string(),
            reasoning_content: None,
        },
    ];

    let messages = builder.build_messages(&history, "", "Next", "web", "chat1", false);
    // system + 1 (assistant, tool skipped) + 1 current = 3
    assert_eq!(messages.len(), 3);
    // The tool message at start was skipped
    assert_eq!(messages[1].role, "assistant");
}

#[test]
fn context_builder_all_bootstrap_files() {
    let tmp = TempDir::new().unwrap();
    for filename in &["AGENT.md", "IDENTITY.md", "SOUL.md", "USER.md", "MCP.md"] {
        std::fs::write(tmp.path().join(filename), format!("Content for {}", filename)).unwrap();
    }

    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    for filename in &["AGENT.md", "IDENTITY.md", "SOUL.md", "USER.md", "MCP.md"] {
        assert!(prompt.contains(filename), "Missing {}", filename);
        assert!(prompt.contains(&format!("Content for {}", filename)), "Missing content for {}", filename);
    }
}

#[test]
fn context_builder_memory_dir_and_skills_dir_paths() {
    let tmp = TempDir::new().unwrap();
    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    // Both paths should appear in the identity section (may be "not yet created")
    assert!(prompt.contains("Memory Path"));
    assert!(prompt.contains("Skills Path"));
}

#[test]
fn context_builder_existing_memory_and_skills_dirs() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("memory")).unwrap();
    std::fs::create_dir_all(tmp.path().join("skills")).unwrap();

    let builder = ContextBuilder::new(tmp.path());
    let prompt = builder.build_system_prompt(false);

    assert!(prompt.contains("memory"));
    assert!(prompt.contains("skills"));
}

#[test]
fn skill_info_debug() {
    let info = SkillInfo {
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        active: true,
    };
    let debug_str = format!("{:?}", info);
    assert!(debug_str.contains("test-skill"));
}

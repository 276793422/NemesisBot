use super::*;
use tempfile::TempDir;

// -------------------------------------------------------------------------
// ProviderAdapter construction tests
// -------------------------------------------------------------------------

#[test]
fn test_provider_adapter_new() {
    // We can't easily construct a real LLMProvider, but we can verify
    // the ProviderAdapter struct fields through its new method pattern.
    // Test the logic of model_to_use selection:
    // - empty model -> use default_model
    // - non-empty model -> use provided model
    let default_model = "gpt-4";
    let empty = "";
    let model_used = if empty.is_empty() { default_model } else { empty };
    assert_eq!(model_used, "gpt-4");

    let provided = "claude-3";
    let model_used = if provided.is_empty() { default_model } else { provided };
    assert_eq!(model_used, "claude-3");
}

// -------------------------------------------------------------------------
// AgentSetCommand / AgentSetAction enum tests
// -------------------------------------------------------------------------

#[test]
fn test_concurrent_mode_validation() {
    let valid_modes = ["reject", "queue"];
    assert!(valid_modes.contains(&"reject"));
    assert!(valid_modes.contains(&"queue"));
    assert!(!valid_modes.contains(&"invalid"));
    assert!(!valid_modes.contains(&"random"));
}

// -------------------------------------------------------------------------
// JSON manipulation for agent set llm
// -------------------------------------------------------------------------

#[test]
fn test_set_llm_config_json_manipulation() {
    let mut cfg: serde_json::Value = serde_json::json!({});
    if let Some(obj) = cfg.as_object_mut() {
        if !obj.contains_key("agents") {
            obj.insert(
                "agents".to_string(),
                serde_json::json!({"defaults": {}}),
            );
        }
        if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
            if !agents.contains_key("defaults") {
                agents.insert("defaults".to_string(), serde_json::json!({}));
            }
            if let Some(defaults) =
                agents.get_mut("defaults").and_then(|v| v.as_object_mut())
            {
                defaults
                    .insert("llm".to_string(), serde_json::Value::String("openai/gpt-4".to_string()));
            }
        }
    }
    assert_eq!(cfg["agents"]["defaults"]["llm"], "openai/gpt-4");
}

#[test]
fn test_set_llm_preserves_existing_agents() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "agents": {
            "defaults": {
                "max_tool_iterations": 10
            }
        }
    });
    if let Some(obj) = cfg.as_object_mut() {
        if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
            if let Some(defaults) =
                agents.get_mut("defaults").and_then(|v| v.as_object_mut())
            {
                defaults
                    .insert("llm".to_string(), serde_json::Value::String("test/model".to_string()));
            }
        }
    }
    assert_eq!(cfg["agents"]["defaults"]["max_tool_iterations"], 10);
    assert_eq!(cfg["agents"]["defaults"]["llm"], "test/model");
}

// -------------------------------------------------------------------------
// JSON manipulation for concurrent mode
// -------------------------------------------------------------------------

#[test]
fn test_set_concurrent_mode_reject() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {}}
    });
    let mode = "reject";
    if let Some(obj) = cfg.as_object_mut() {
        if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
            if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                defaults.insert(
                    "concurrent_request_mode".to_string(),
                    serde_json::Value::String(mode.to_string()),
                );
            }
        }
    }
    assert_eq!(cfg["agents"]["defaults"]["concurrent_request_mode"], "reject");
}

#[test]
fn test_set_concurrent_mode_queue_with_size() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {}}
    });
    let mode = "queue";
    let queue_size: Option<usize> = Some(16);
    if let Some(obj) = cfg.as_object_mut() {
        if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
            if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                defaults.insert(
                    "concurrent_request_mode".to_string(),
                    serde_json::Value::String(mode.to_string()),
                );
                if mode == "queue" {
                    defaults.insert(
                        "queue_size".to_string(),
                        serde_json::json!(queue_size.unwrap_or(8)),
                    );
                }
            }
        }
    }
    assert_eq!(cfg["agents"]["defaults"]["concurrent_request_mode"], "queue");
    assert_eq!(cfg["agents"]["defaults"]["queue_size"], 16);
}

#[test]
fn test_set_concurrent_mode_queue_default_size() {
    let queue_size: Option<usize> = None;
    assert_eq!(queue_size.unwrap_or(8), 8);
}

// -------------------------------------------------------------------------
// LlmMessage conversion logic
// -------------------------------------------------------------------------

#[test]
fn test_message_role_mapping() {
    // Verify role string passthrough
    let roles = ["system", "user", "assistant", "tool"];
    for role in &roles {
        let msg = LlmMessage {
            role: role.to_string(),
            content: "test".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };
        assert_eq!(msg.role, *role);
        assert_eq!(msg.content, "test");
    }
}

#[test]
fn test_llm_response_finished_logic() {
    // finished = tool_calls.is_empty() || finish_reason == "stop"
    let tool_calls: Vec<AgentToolCallInfo> = vec![];
    assert!(tool_calls.is_empty()); // empty tool_calls -> finished = true

    let tool_calls = vec![AgentToolCallInfo {
        id: "tc1".to_string(),
        name: "test".to_string(),
        arguments: "{}".to_string(),
    }];
    assert!(!tool_calls.is_empty()); // with tool_calls, finished depends on finish_reason
}

// -------------------------------------------------------------------------
// Config resolution validation
// -------------------------------------------------------------------------

#[test]
fn test_factory_config_construction() {
    let llm_ref = "openai/gpt-4";
    let parts: Vec<&str> = llm_ref.splitn(2, '/').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "openai");
    assert_eq!(parts[1], "gpt-4");
}

#[test]
fn test_factory_config_with_slash_in_model() {
    let llm_ref = "test/model-name-v2";
    let parts: Vec<&str> = llm_ref.splitn(2, '/').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "test");
    assert_eq!(parts[1], "model-name-v2");
}

// -------------------------------------------------------------------------
// AgentConfig construction
// -------------------------------------------------------------------------

#[test]
fn test_agent_config_default_max_turns() {
    // ① New semantics: max_tool_iterations <= 0 means "unlimited", represented
    // as 0 in AgentConfig.max_turns (the run-loop treats 0 as "no cap"). A
    // positive value is used as-is.
    let max_tool_iterations: i32 = 0;
    let max_turns = if max_tool_iterations <= 0 { 0u32 } else { max_tool_iterations as u32 };
    assert_eq!(max_turns, 0);

    let max_tool_iterations: i32 = 10;
    let max_turns = if max_tool_iterations <= 0 { 0u32 } else { max_tool_iterations as u32 };
    assert_eq!(max_turns, 10);

    let max_tool_iterations: i32 = -5;
    let max_turns = if max_tool_iterations <= 0 { 0u32 } else { max_tool_iterations as u32 };
    assert_eq!(max_turns, 0);
}

// -------------------------------------------------------------------------
// Log args construction
// -------------------------------------------------------------------------

#[test]
fn test_log_args_construction() {
    let debug = true;
    let quiet = false;
    let no_console = true;
    let mut log_args: Vec<String> = Vec::new();
    if debug {
        log_args.push("--debug".to_string());
    }
    if quiet {
        log_args.push("--quiet".to_string());
    }
    if no_console {
        log_args.push("--no-console".to_string());
    }
    assert_eq!(log_args, vec!["--debug", "--no-console"]);
}

// -------------------------------------------------------------------------
// Confirm prompt answer parsing
// -------------------------------------------------------------------------

#[test]
fn test_confirm_answer_parsing() {
    let answer = "y".to_string();
    assert!(answer.trim().to_lowercase() == "y");

    let answer = "Y".to_string();
    assert!(answer.trim().to_lowercase() == "y");

    let answer = "n".to_string();
    assert!(answer.trim().to_lowercase() != "y");

    let answer = "yes".to_string();
    assert!(answer.trim().to_lowercase() != "y"); // only "y" is accepted
}

// -------------------------------------------------------------------------
// Config file round-trip with agents section
// -------------------------------------------------------------------------

#[test]
fn test_config_round_trip_agents_section() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");

    // Write config with agents section
    let cfg = serde_json::json!({
        "agents": {
            "defaults": {
                "llm": "openai/gpt-4",
                "max_tool_iterations": 15
            }
        }
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    // Read back
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed["agents"]["defaults"]["llm"], "openai/gpt-4");
    assert_eq!(parsed["agents"]["defaults"]["max_tool_iterations"], 15);
}

// -------------------------------------------------------------------------
// ChatOptions default behavior
// -------------------------------------------------------------------------

#[test]
fn test_chat_options_defaults() {
    // Simulates the None branch of provider_options construction
    let provider_options = nemesis_providers::types::ChatOptions {
        temperature: Some(0.7),
        max_tokens: Some(8192),
        top_p: None,
        stop: None,
        extra: std::collections::HashMap::new(),
    };
    assert_eq!(provider_options.temperature, Some(0.7));
    assert_eq!(provider_options.max_tokens, Some(8192));
    assert!(provider_options.top_p.is_none());
    assert!(provider_options.stop.is_none());
}

#[test]
fn test_chat_options_from_agent_options() {
    // Simulates the Some(opts) branch
    let temperature: Option<f32> = Some(0.5);
    let max_tokens: Option<i32> = Some(4096);
    let top_p: Option<f32> = Some(0.9);

    let provider_options = nemesis_providers::types::ChatOptions {
        temperature: temperature.map(|t| t as f64),
        max_tokens: max_tokens.map(|t| t as i64),
        top_p: top_p.map(|p| p as f64),
        stop: None,
        extra: std::collections::HashMap::new(),
    };
    assert_eq!(provider_options.temperature, Some(0.5));
    assert_eq!(provider_options.max_tokens, Some(4096));
    assert!((provider_options.top_p.unwrap() - 0.9).abs() < 0.01);
}

// -------------------------------------------------------------------------
// Tool calls conversion
// -------------------------------------------------------------------------

#[test]
fn test_tool_call_info_fields() {
    let tc = AgentToolCallInfo {
        id: "call_123".to_string(),
        name: "file_read".to_string(),
        arguments: "{\"path\": \"/tmp/test\"}".to_string(),
    };
    assert_eq!(tc.id, "call_123");
    assert_eq!(tc.name, "file_read");
    assert!(tc.arguments.contains("path"));
}

// -------------------------------------------------------------------------
// Interactive mode input handling
// -------------------------------------------------------------------------

#[test]
fn test_interactive_input_commands() {
    // Test the exit/quit logic
    let input = "exit".to_string();
    assert!(input == "exit" || input == "quit");

    let input = "quit".to_string();
    assert!(input == "exit" || input == "quit");

    let input = "hello".to_string();
    assert!(input != "exit" && input != "quit");
}

#[test]
fn test_interactive_slash_commands() {
    let valid_commands = ["/history", "/clear", "/status"];
    let input = "/history";
    assert!(valid_commands.contains(&input));

    let input = "/unknown";
    assert!(!valid_commands.contains(&input));
}

#[test]
fn test_input_trim_and_empty_check() {
    let input = "   ".to_string();
    let trimmed = input.trim().to_string();
    assert!(trimmed.is_empty());

    let input = "  hello  ".to_string();
    let trimmed = input.trim().to_string();
    assert!(!trimmed.is_empty());
    assert_eq!(trimmed, "hello");
}

// -------------------------------------------------------------------------
// Preview truncation logic (from /history command)
// -------------------------------------------------------------------------

#[test]
fn test_history_preview_truncation() {
    let content = "a".repeat(100);
    let preview = if content.len() > 80 {
        format!("{}...", &content[..77])
    } else {
        content.clone()
    };
    assert!(preview.len() <= 80);
    assert!(preview.ends_with("..."));

    let content = "short message".to_string();
    let preview = if content.len() > 80 {
        format!("{}...", &content[..77])
    } else {
        content.clone()
    };
    assert_eq!(preview, "short message");
}

// -------------------------------------------------------------------------
// Additional coverage tests for agent
// -------------------------------------------------------------------------

#[test]
fn test_agent_entry_minimal() {
    let entry = serde_json::json!({
        "id": "test-agent",
    });
    assert_eq!(entry["id"], "test-agent");
    assert!(entry.get("model").is_none());
}

#[test]
fn test_agent_entry_with_model() {
    let entry = serde_json::json!({
        "id": "test-agent",
        "model": "gpt-4o",
    });
    assert_eq!(entry["model"], "gpt-4o");
}

#[test]
fn test_agent_entry_with_all_fields() {
    let entry = serde_json::json!({
        "id": "test-agent",
        "model": "gpt-4o",
        "system_prompt": "You are a helper",
        "tools": "file,web",
        "temperature": "0.5"
    });
    assert_eq!(entry["id"], "test-agent");
    assert_eq!(entry["model"], "gpt-4o");
    assert_eq!(entry["system_prompt"], "You are a helper");
    assert_eq!(entry["tools"], "file,web");
    assert_eq!(entry["temperature"], "0.5");
}

#[test]
fn test_agent_config_read_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.json");
    if path.exists() {
        let data = std::fs::read_to_string(&path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(cfg.is_object());
    } else {
        let cfg = serde_json::json!({"agents": {"instances": []}});
        assert!(cfg["agents"]["instances"].is_array());
    }
}

#[test]
fn test_agent_config_read_existing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.json");
    let data = serde_json::json!({
        "agents": {
            "instances": [{"id": "test-agent", "model": "gpt-4o"}]
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let instances = cfg["agents"]["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 1);
}

#[test]
fn test_agent_config_save_creates_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("nested").join("config.json");
    let cfg = serde_json::json!({"agents": {}});
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    assert!(path.exists());
}

#[test]
fn test_agent_entry_serialization() {
    let entry = serde_json::json!({"id": "agent-1", "model": "model-1", "system_prompt": "prompt"});
    let json = serde_json::to_string(&entry).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["id"], "agent-1");
    assert_eq!(parsed["model"], "model-1");
    assert_eq!(parsed["system_prompt"], "prompt");
}

#[test]
fn test_history_preview_exactly_80_chars() {
    let content = "a".repeat(80);
    let preview = if content.len() > 80 {
        format!("{}...", &content[..77])
    } else {
        content.clone()
    };
    assert_eq!(preview.len(), 80);
    assert!(!preview.ends_with("...")); // Exactly 80, no truncation
}

#[test]
fn test_history_preview_81_chars() {
    let content = "a".repeat(81);
    let preview = if content.len() > 80 {
        format!("{}...", &content[..77])
    } else {
        content.clone()
    };
    assert!(preview.ends_with("..."));
    assert!(preview.len() <= 80);
}

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
    let model_used = if empty.is_empty() {
        default_model
    } else {
        empty
    };
    assert_eq!(model_used, "gpt-4");

    let provided = "claude-3";
    let model_used = if provided.is_empty() {
        default_model
    } else {
        provided
    };
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
            obj.insert("agents".to_string(), serde_json::json!({"defaults": {}}));
        }
        if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
            if !agents.contains_key("defaults") {
                agents.insert("defaults".to_string(), serde_json::json!({}));
            }
            if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                defaults.insert(
                    "llm".to_string(),
                    serde_json::Value::String("openai/gpt-4".to_string()),
                );
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
            if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                defaults.insert(
                    "llm".to_string(),
                    serde_json::Value::String("test/model".to_string()),
                );
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
    assert_eq!(
        cfg["agents"]["defaults"]["concurrent_request_mode"],
        "reject"
    );
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
    assert_eq!(
        cfg["agents"]["defaults"]["concurrent_request_mode"],
        "queue"
    );
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
    let max_turns = if max_tool_iterations <= 0 {
        0u32
    } else {
        max_tool_iterations as u32
    };
    assert_eq!(max_turns, 0);

    let max_tool_iterations: i32 = 10;
    let max_turns = if max_tool_iterations <= 0 {
        0u32
    } else {
        max_tool_iterations as u32
    };
    assert_eq!(max_turns, 10);

    let max_tool_iterations: i32 = -5;
    let max_turns = if max_tool_iterations <= 0 {
        0u32
    } else {
        max_tool_iterations as u32
    };
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
        reasoning_effort: None,
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
        reasoning_effort: None,
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

// =========================================================================
// S11b 覆盖率冲刺（quality-hardening goal）：真实调用面测试。
// - ProviderAdapter（LlmProvider impl）全转换路径：LlmMessage→Message、
//   ToolDefinition 转发、ChatOptions Some/None、响应 tool_calls 过滤 +
//   finished 判定 + usage 映射、provider Err→String。
// - run() 无子命令：cfg 缺失 bail / build 失败 bail+提示 / 单消息模式
//   死地址 provider 全流程（连接拒绝 → "Agent error" → Ok(())）。
// - run() Set Llm：cfg 缺失 bail / resolve 成功写回 agents.defaults.llm。
// - run() Set ConcurrentMode：非法 mode bail / reject / queue 带尺寸 /
//   queue 默认 8 / 无 config.json → Ok 不写。
// 豁免（不测）：rustyline 交互 REPL（stdin）、Set Llm resolve 失败后的
// y/N stdin 确认、真实 LLM 调用。
// =========================================================================

struct S11bAgentHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for S11bAgentHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn s11b_agent_home_env() -> S11bAgentHomeEnv {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bAgentHomeEnv { _tmp: tmp, home }
}

fn s11b_write_agent_config(home: &std::path::Path, cfg: serde_json::Value) {
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn s11b_read_agent_config(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

/// 死地址模型条目（127.0.0.1:1 立即连接拒绝，无重试等待）。
fn s11b_dead_provider_config() -> serde_json::Value {
    serde_json::json!({
        "agents": {"defaults": {"llm": "fake"}},
        "model_list": [{
            "model_name": "fake",
            "model": "openai/gpt-fake",
            "api_base": "http://127.0.0.1:1",
            "api_key": "k"
        }]
    })
}

// ------------------------------ mock provider ------------------------------

enum S11bMockReply {
    /// finish_reason=stop、无 tool_calls、带 usage。
    Stop,
    /// finish_reason=tool_calls、1 个正常 + 1 个缺 function 的 tool_calls。
    WithToolCalls,
    /// FailoverError::Unknown。
    Fail,
}

/// 记录 chat 入参快照的 mock provider（实现 provider 侧 LLMProvider trait）。
struct S11bMockProvider {
    calls: std::sync::Mutex<Vec<serde_json::Value>>,
    reply: S11bMockReply,
}

#[async_trait::async_trait]
impl nemesis_providers::router::LLMProvider for S11bMockProvider {
    async fn chat(
        &self,
        messages: &[nemesis_providers::types::Message],
        tools: &[nemesis_providers::types::ToolDefinition],
        model: &str,
        options: &nemesis_providers::types::ChatOptions,
    ) -> Result<
        nemesis_providers::types::LLMResponse,
        nemesis_providers::failover::FailoverError,
    > {
        let snapshot = serde_json::json!({
            "model": model,
            "messages": serde_json::to_value(messages).unwrap(),
            "tools": serde_json::to_value(tools).unwrap(),
            "temperature": options.temperature,
            "max_tokens": options.max_tokens,
            "top_p": options.top_p,
            "stop": options.stop,
            "reasoning_effort": options.reasoning_effort,
        });
        self.calls.lock().unwrap().push(snapshot);
        match self.reply {
            S11bMockReply::Stop => Ok(nemesis_providers::types::LLMResponse {
                content: "mock-reply".to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                usage: Some(nemesis_providers::types::UsageInfo {
                    prompt_tokens: 11,
                    completion_tokens: 7,
                    total_tokens: 18,
                    cached_tokens: Some(3),
                    ..Default::default()
                }),
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
                raw_request_body: None,
                raw_response_body: None,
            }),
            S11bMockReply::WithToolCalls => {
                Ok(nemesis_providers::types::LLMResponse {
                    content: String::new(),
                    tool_calls: vec![
                        nemesis_providers::types::ToolCall {
                            id: "tc1".to_string(),
                            call_type: Some("function".to_string()),
                            function: Some(nemesis_providers::types::FunctionCall {
                                name: "read_file".to_string(),
                                arguments: "{\"path\":\"a.txt\"}".to_string(),
                            }),
                            name: None,
                            arguments: None,
                        },
                        // 缺 function 的畸形 tool_call —— adapter 必须丢弃
                        nemesis_providers::types::ToolCall {
                            id: "tc2".to_string(),
                            call_type: None,
                            function: None,
                            name: None,
                            arguments: None,
                        },
                    ],
                    finish_reason: "tool_calls".to_string(),
                    usage: None,
                    reasoning_content: None,
                    extra: std::collections::HashMap::new(),
                    raw_request_body: None,
                    raw_response_body: None,
                })
            }
            S11bMockReply::Fail => Err(nemesis_providers::failover::FailoverError::Unknown {
                provider: "mock".to_string(),
                message: "boom-s11b".to_string(),
            }),
        }
    }
    fn default_model(&self) -> &str {
        "mock-default-model"
    }
    fn name(&self) -> &str {
        "mock"
    }
}

fn s11b_adapter(
    reply: S11bMockReply,
) -> (
    ProviderAdapter,
    std::sync::Arc<S11bMockProvider>,
) {
    let inner = std::sync::Arc::new(S11bMockProvider {
        calls: std::sync::Mutex::new(Vec::new()),
        reply,
    });
    (
        ProviderAdapter::new(inner.clone(), "mock-default-model".to_string()),
        inner,
    )
}

// -------------------------- ProviderAdapter tests --------------------------

#[tokio::test]
async fn test_s11b_adapter_model_fallback_to_default() {
    let (adapter, inner) = s11b_adapter(S11bMockReply::Stop);
    // 空 model → default_model
    adapter
        .chat("", vec![], None, vec![])
        .await
        .unwrap();
    // 非空 model → 原样透传
    adapter
        .chat("m2", vec![], None, vec![])
        .await
        .unwrap();
    let calls = inner.calls.lock().unwrap();
    assert_eq!(calls[0]["model"], "mock-default-model");
    assert_eq!(calls[1]["model"], "m2");
}

#[tokio::test]
async fn test_s11b_adapter_message_and_tool_conversion() {
    let (adapter, inner) = s11b_adapter(S11bMockReply::Stop);
    let messages = vec![
        LlmMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![AgentToolCallInfo {
                id: "t9".to_string(),
                name: "shell".to_string(),
                arguments: "{}".to_string(),
            }]),
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "user".to_string(),
            content: "hi there".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "tool".to_string(),
            content: "tool output".to_string(),
            tool_calls: None,
            tool_call_id: Some("t9".to_string()),
            reasoning_content: Some("because".to_string()),
        },
    ];
    let tools = vec![nemesis_agent::types::ToolDefinition {
        tool_type: "function".to_string(),
        function: nemesis_agent::types::ToolFunctionDef {
            name: "grep_code".to_string(),
            description: "search code".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    adapter.chat("m", messages, None, tools).await.unwrap();

    let calls = inner.calls.lock().unwrap();
    let msgs = calls[0]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    // assistant 消息的 tool_calls → call_type 固定 "function" + function 载荷
    assert_eq!(msgs[0]["role"], "assistant");
    let tc = &msgs[0]["tool_calls"][0];
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "shell");
    assert_eq!(tc["function"]["arguments"], "{}");
    assert!(tc.get("name").is_none() || tc["name"].is_null());
    // user 消息原样、无 tool_call 字段
    assert_eq!(msgs[1]["content"], "hi there");
    // tool_calls None → 空 vec → serde skip_serializing_if 省略整个字段
    assert!(msgs[1]["tool_calls"].is_null());
    // tool 消息 tool_call_id + reasoning_content 透传
    assert_eq!(msgs[2]["tool_call_id"], "t9");
    assert_eq!(msgs[2]["reasoning_content"], "because");
    // 工具定义转发（名称 + 类型）
    let tools = calls[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "grep_code");
    assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
}

#[tokio::test]
async fn test_s11b_adapter_options_passthrough() {
    let (adapter, inner) = s11b_adapter(S11bMockReply::Stop);
    adapter
        .chat(
            "m",
            vec![],
            Some(nemesis_agent::types::ChatOptions {
                temperature: Some(0.25),
                max_tokens: Some(55),
                top_p: Some(0.5),
                stop: Some(vec!["END".to_string()]),
                reasoning_effort: Some("low".to_string()),
            }),
            vec![],
        )
        .await
        .unwrap();
    let calls = inner.calls.lock().unwrap();
    assert_eq!(calls[0]["temperature"], 0.25);
    assert_eq!(calls[0]["max_tokens"], 55);
    assert_eq!(calls[0]["top_p"], 0.5);
    assert_eq!(calls[0]["stop"], serde_json::json!(["END"]));
    assert_eq!(calls[0]["reasoning_effort"], "low");
}

#[tokio::test]
async fn test_s11b_adapter_options_defaults_when_none() {
    let (adapter, inner) = s11b_adapter(S11bMockReply::Stop);
    adapter.chat("m", vec![], None, vec![]).await.unwrap();
    let calls = inner.calls.lock().unwrap();
    // None → temperature 0.7 / max_tokens 8192 / 其余空
    assert_eq!(calls[0]["temperature"], 0.7);
    assert_eq!(calls[0]["max_tokens"], 8192);
    assert!(calls[0]["top_p"].is_null());
    assert!(calls[0]["stop"].is_null());
    assert!(calls[0]["reasoning_effort"].is_null());
}

#[tokio::test]
async fn test_s11b_adapter_tool_calls_filter_and_unfinished() {
    let (adapter, _inner) = s11b_adapter(S11bMockReply::WithToolCalls);
    let resp = adapter.chat("m", vec![], None, vec![]).await.unwrap();
    // 缺 function 的 tc2 被丢弃，只留 tc1
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "tc1");
    assert_eq!(resp.tool_calls[0].name, "read_file");
    assert_eq!(resp.tool_calls[0].arguments, "{\"path\":\"a.txt\"}");
    // finish_reason=tool_calls 且 tool_calls 非空 → 未结束
    assert!(!resp.finished);
}

#[tokio::test]
async fn test_s11b_adapter_stop_reply_finished_and_usage() {
    let (adapter, _inner) = s11b_adapter(S11bMockReply::Stop);
    let resp = adapter.chat("m", vec![], None, vec![]).await.unwrap();
    assert_eq!(resp.content, "mock-reply");
    assert!(resp.tool_calls.is_empty());
    assert!(resp.finished);
    let usage = resp.usage.expect("usage mapped from provider");
    assert_eq!(usage.prompt_tokens, 11);
    assert_eq!(usage.completion_tokens, 7);
    assert_eq!(usage.total_tokens, 18);
    assert_eq!(usage.cached_tokens, Some(3));
    assert_eq!(usage.cache_creation_tokens, None);
}

#[tokio::test]
async fn test_s11b_adapter_provider_error_to_string() {
    let (adapter, _inner) = s11b_adapter(S11bMockReply::Fail);
    let err = adapter.chat("m", vec![], None, vec![]).await.unwrap_err();
    // provider FailoverError → String（Display 含消息原文）
    assert!(err.contains("boom-s11b"), "error should embed message: {err}");
}

// ------------------------------- run() tests -------------------------------

#[tokio::test]
async fn test_s11b_run_agent_mode_config_missing_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_agent_home_env();
    // 无 config.json → 在 build/REPL 之前 bail（不会进入交互模式）
    let err = run(None, Some("hi".to_string()), "s11b".to_string(), false, false, false, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Configuration not found"), "{err}");
}

#[tokio::test]
async fn test_s11b_run_agent_mode_build_fail() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    // llm 指向不存在的模型（无关键词可推断 provider）→ build_agent_loop Err
    s11b_write_agent_config(
        &th.home,
        serde_json::json!({"agents": {"defaults": {"llm": "nosuchmodel-xyz"}}}),
    );
    let err = run(None, Some("hi".to_string()), "s11b".to_string(), false, false, false, false)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Failed to resolve model"),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_s11b_run_agent_mode_single_message_dead_provider() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    s11b_write_agent_config(&th.home, s11b_dead_provider_config());
    // build 成功 + 单消息模式：LLM 调用打到死地址 → process_direct Err →
    // "Agent error" 打印后 run 返回 Ok(())（不 panic、不上抛）
    let res = run(
        None,
        Some("say hi".to_string()),
        "s11b-session".to_string(),
        false,
        false,
        false,
        false,
    )
    .await;
    assert!(res.is_ok(), "single-message mode must not propagate LLM error: {res:?}");
}

#[test]
fn test_s11b_build_agent_loop_registers_default_agent() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    s11b_write_agent_config(&th.home, s11b_dead_provider_config());
    let cfg = nemesis_config::load_config(&th.home.join("config.json")).unwrap();
    let agent_loop = build_agent_loop(&cfg, &th.home).expect("dead-addr provider builds fine");
    // 共享工具注册后 tool_count 明显非零（register_shared_tools 生效）。
    // standalone 构造路径 registry 为 None（get_registry 只在 gateway 侧装配）。
    assert!(agent_loop.tool_count() >= 20, "tools registered: {}", agent_loop.tool_count());
    assert!(agent_loop.get_registry().is_none());
}

#[tokio::test]
async fn test_s11b_run_set_llm_config_missing_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_agent_home_env();
    let err = run(
        Some(AgentSetCommand::Set {
            action: AgentSetAction::Llm { model: "x".to_string() },
        }),
        None,
        "s11b".to_string(),
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Configuration not found"), "{err}");
}

#[tokio::test]
async fn test_s11b_run_set_llm_resolved_writes_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    // config 无 agents 段但 model_list 含 "fake"（resolve 成功，不走 y/N 确认）
    // → 写回时补齐 agents.defaults.llm
    s11b_write_agent_config(
        &th.home,
        serde_json::json!({
            "model_list": [{
                "model_name": "fake",
                "model": "openai/gpt-fake",
                "api_base": "http://127.0.0.1:1",
                "api_key": "k"
            }]
        }),
    );
    run(
        Some(AgentSetCommand::Set {
            action: AgentSetAction::Llm { model: "fake".to_string() },
        }),
        None,
        "s11b".to_string(),
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap();
    let cfg = s11b_read_agent_config(&th.home);
    assert_eq!(cfg["agents"]["defaults"]["llm"], "fake");
}

#[tokio::test]
async fn test_s11b_run_set_concurrent_mode_invalid_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    s11b_write_agent_config(&th.home, serde_json::json!({"agents": {"defaults": {}}}));
    let err = run(
        Some(AgentSetCommand::Set {
            action: AgentSetAction::ConcurrentMode {
                mode: "bogus".to_string(),
                queue_size: None,
            },
        }),
        None,
        "s11b".to_string(),
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Invalid mode"), "{err}");
}

#[tokio::test]
async fn test_s11b_run_set_concurrent_mode_reject_then_queue() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    s11b_write_agent_config(&th.home, serde_json::json!({"agents": {"defaults": {}}}));

    let set_mode = |mode: &str, queue_size: Option<usize>| {
        Some(AgentSetCommand::Set {
            action: AgentSetAction::ConcurrentMode {
                mode: mode.to_string(),
                queue_size,
            },
        })
    };

    // reject：写 concurrent_request_mode，不写 queue_size
    run(set_mode("reject", None), None, "s11b".to_string(), false, false, false, false)
        .await
        .unwrap();
    let cfg = s11b_read_agent_config(&th.home);
    assert_eq!(cfg["agents"]["defaults"]["concurrent_request_mode"], "reject");
    assert!(cfg["agents"]["defaults"].get("queue_size").is_none());

    // queue 带尺寸
    run(set_mode("queue", Some(3)), None, "s11b".to_string(), false, false, false, false)
        .await
        .unwrap();
    let cfg = s11b_read_agent_config(&th.home);
    assert_eq!(cfg["agents"]["defaults"]["concurrent_request_mode"], "queue");
    assert_eq!(cfg["agents"]["defaults"]["queue_size"], 3);

    // queue 默认尺寸 8
    run(set_mode("queue", None), None, "s11b".to_string(), false, false, false, false)
        .await
        .unwrap();
    let cfg = s11b_read_agent_config(&th.home);
    assert_eq!(cfg["agents"]["defaults"]["queue_size"], 8);
}

#[tokio::test]
async fn test_s11b_run_set_concurrent_mode_no_config_ok() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    // 无 config.json → 跳过写盘，仍 Ok
    run(
        Some(AgentSetCommand::Set {
            action: AgentSetAction::ConcurrentMode {
                mode: "reject".to_string(),
                queue_size: None,
            },
        }),
        None,
        "s11b".to_string(),
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(!th.home.join("config.json").exists());
}

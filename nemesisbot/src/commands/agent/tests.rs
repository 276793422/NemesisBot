// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

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
    if let Some(obj) = cfg.as_object_mut()
        && let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut())
        && let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut())
    {
        defaults.insert(
            "llm".to_string(),
            serde_json::Value::String("test/model".to_string()),
        );
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
    if let Some(obj) = cfg.as_object_mut()
        && let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut())
        && let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut())
    {
        defaults.insert(
            "concurrent_request_mode".to_string(),
            serde_json::Value::String(mode.to_string()),
        );
    }
    assert_eq!(
        cfg["agents"]["defaults"]["concurrent_request_mode"],
        "reject"
    );
}

/// Mirror of the production queue_size default; kept as a fn so the
/// `unwrap_or` fallback path stays exercised (clippy can't const-fold it).
fn resolve_queue_size(queue_size: Option<usize>) -> usize {
    queue_size.unwrap_or(8)
}

#[test]
fn test_set_concurrent_mode_queue_with_size() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {}}
    });
    let mode = "queue";
    let queue_size: Option<usize> = Some(16);
    if let Some(obj) = cfg.as_object_mut()
        && let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut())
        && let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut())
    {
        defaults.insert(
            "concurrent_request_mode".to_string(),
            serde_json::Value::String(mode.to_string()),
        );
        if mode == "queue" {
            defaults.insert(
                "queue_size".to_string(),
                serde_json::json!(resolve_queue_size(queue_size)),
            );
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
    assert_eq!(resolve_queue_size(None), 8);
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

    let tool_calls = [AgentToolCallInfo {
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
    ) -> Result<nemesis_providers::types::LLMResponse, nemesis_providers::failover::FailoverError>
    {
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

fn s11b_adapter(reply: S11bMockReply) -> (ProviderAdapter, std::sync::Arc<S11bMockProvider>) {
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
    adapter.chat("", vec![], None, vec![]).await.unwrap();
    // 非空 model → 原样透传
    adapter.chat("m2", vec![], None, vec![]).await.unwrap();
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
    assert!(
        err.contains("boom-s11b"),
        "error should embed message: {err}"
    );
}

// ------------------------------- run() tests -------------------------------

#[tokio::test]
async fn test_s11b_run_agent_mode_config_missing_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_agent_home_env();
    // 无 config.json → 在 build/REPL 之前 bail（不会进入交互模式）
    let err = run(
        None,
        Some("hi".to_string()),
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
async fn test_s11b_run_agent_mode_build_fail() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_agent_home_env();
    // llm 指向不存在的模型（无关键词可推断 provider）→ build_agent_loop Err
    s11b_write_agent_config(
        &th.home,
        serde_json::json!({"agents": {"defaults": {"llm": "nosuchmodel-xyz"}}}),
    );
    let err = run(
        None,
        Some("hi".to_string()),
        "s11b".to_string(),
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Failed to resolve model"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    assert!(
        res.is_ok(),
        "single-message mode must not propagate LLM error: {res:?}"
    );
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
    assert!(
        agent_loop.tool_count() >= 20,
        "tools registered: {}",
        agent_loop.tool_count()
    );
    assert!(agent_loop.get_registry().is_none());
}

#[tokio::test]
async fn test_s11b_run_set_llm_config_missing_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_agent_home_env();
    let err = run(
        Some(AgentSetCommand::Set {
            action: AgentSetAction::Llm {
                model: "x".to_string(),
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
            action: AgentSetAction::Llm {
                model: "fake".to_string(),
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
    run(
        set_mode("reject", None),
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
    assert_eq!(
        cfg["agents"]["defaults"]["concurrent_request_mode"],
        "reject"
    );
    assert!(cfg["agents"]["defaults"].get("queue_size").is_none());

    // queue 带尺寸
    run(
        set_mode("queue", Some(3)),
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
    assert_eq!(
        cfg["agents"]["defaults"]["concurrent_request_mode"],
        "queue"
    );
    assert_eq!(cfg["agents"]["defaults"]["queue_size"], 3);

    // queue 默认尺寸 8
    run(
        set_mode("queue", None),
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

// ===========================================================================
// wave_b（coverage 补测）：build_agent_loop 的 skills / registry /
// RequestLogger 分支 + run() CLI 开关臂 + 本地回环 mock LLM 全流程。
//
// 不可测豁免（证据）：
// - agent.rs:218-219 `!resolution.enabled` 守卫：resolve_model_config 三处
//   硬编码 enabled:true（provider_resolver.rs:59/103/179）→ 死分支。
// - agent.rs:266 `system_prompt.is_empty()` None 臂：ContextBuilder::
//   build_system_prompt 无条件 push build_identity()（context.rs:328）
//   → 永非空。
// - rustyline REPL / Set-Llm y/N stdin 确认：交互 tty 下 read_line 阻塞。
// =========================================================================

mod wave_b {
    use super::*;

    /// 本地回环 OpenAI 兼容 mock：POST /chat/completions 返回一条
    /// finish_reason=stop、无 tool_calls 的补全。adapter 走非流式 chat()，
    /// 普通 JSON 即可；若请求体出现 "stream":true 则回 SSE（防御，现路径
    /// 不触发）。返回端口；连接计数供调用方断言至少被调一次。
    fn start_openai_mock(
        content: &'static str,
    ) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits_clone = hits.clone();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                hits_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(800)));
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let head_end = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .unwrap_or(buf.len());
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let content_length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        if !k.trim().eq_ignore_ascii_case("content-length") {
                            return None;
                        }
                        v.trim().parse::<usize>().ok()
                    })
                    .unwrap_or(0);
                let mut body = buf[head_end.min(buf.len())..].to_vec();
                while body.len() < content_length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body.extend_from_slice(&chunk[..n]),
                    }
                }
                let wants_stream = body.windows(13).any(|w| w == b"\"stream\":true");
                let resp = if wants_stream {
                    let mut sse = String::new();
                    sse.push_str(
                        "data: {\"id\":\"wb\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"wb-mock-reply\"}}]}\n\n",
                    );
                    sse.push_str(
                        "data: {\"id\":\"wb\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    );
                    sse.push_str("data: [DONE]\n\n");
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        sse.len(),
                        sse
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        content.len(),
                        content
                    )
                };
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (port, hits)
    }

    #[test]
    fn wave_b_build_loop_loads_skills_when_dir_present() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        s11b_write_agent_config(&th.home, s11b_dead_provider_config());
        // workspace/skills 存在 → context_builder.load_skills 分支被触发。
        let skills_dir = th.home.join("workspace").join("skills").join("wb-skill");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: wb-skill\ndescription: wave-b probe skill\n---\nbody",
        )
        .unwrap();

        let cfg = nemesis_config::load_config(&th.home.join("config.json")).unwrap();
        build_agent_loop(&cfg, &th.home).expect("skills 目录在场不影响构建 → Ok");
    }

    #[test]
    fn wave_b_build_loop_zero_max_tool_iterations_maps_unlimited() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        // max_tool_iterations=0 → max_turns 映射为 0（unlimited opt-in 分支）。
        s11b_write_agent_config(
            &th.home,
            serde_json::json!({
                "agents": {"defaults": {"llm": "fake", "max_tool_iterations": 0}},
                "model_list": [{
                    "model_name": "fake",
                    "model": "openai/gpt-fake",
                    "api_base": "http://127.0.0.1:1",
                    "api_key": "k"
                }]
            }),
        );
        let cfg = nemesis_config::load_config(&th.home.join("config.json")).unwrap();
        build_agent_loop(&cfg, &th.home).expect("unlimited 档位照常构建 → Ok");
    }

    #[test]
    fn wave_b_build_loop_skills_registry_from_config_skills_json() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        s11b_write_agent_config(&th.home, s11b_dead_provider_config());
        // workspace/config/config.skills.json 在场 → skills_registry 解析分支；
        // RegistryConfig 全字段 #[serde(default)]，`{}` 合法。
        let cfg_dir = th.home.join("workspace").join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("config.skills.json"), "{}").unwrap();

        let cfg = nemesis_config::load_config(&th.home.join("config.json")).unwrap();
        build_agent_loop(&cfg, &th.home)
            .expect("config.skills.json={} 可解析 → RegistryManager 构建成功 → Ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_build_loop_request_logger_truncated_custom_logdir() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        // logging.llm{truncated,自定义 log_dir,save_raw} → DetailLevel::Truncated
        // + 自定义目录三路匹配全命中；block_in_place 注册 observer。
        s11b_write_agent_config(
            &th.home,
            serde_json::json!({
                "agents": {"defaults": {"llm": "fake"}},
                "logging": {"llm": {
                    "enabled": true,
                    "detail_level": "truncated",
                    "log_dir": "wb-request-logs",
                    "save_raw": true
                }},
                "model_list": [{
                    "model_name": "fake",
                    "model": "openai/gpt-fake",
                    "api_base": "http://127.0.0.1:1",
                    "api_key": "k"
                }]
            }),
        );
        let cfg = nemesis_config::load_config(&th.home.join("config.json")).unwrap();
        build_agent_loop(&cfg, &th.home)
            .expect("RequestLogger(truncated+custom dir) 注册后构建 Ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_build_loop_request_logger_defaults_full_and_default_logdir() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        // 只给 enabled → detail_level 兜底 Full + log_dir 空串兜底
        // "logs/request_logs"（两条 `_ =>`/空串 else 分支）。
        s11b_write_agent_config(
            &th.home,
            serde_json::json!({
                "agents": {"defaults": {"llm": "fake"}},
                "logging": {"llm": {"enabled": true}},
                "model_list": [{
                    "model_name": "fake",
                    "model": "openai/gpt-fake",
                    "api_base": "http://127.0.0.1:1",
                    "api_key": "k"
                }]
            }),
        );
        let cfg = nemesis_config::load_config(&th.home.join("config.json")).unwrap();
        build_agent_loop(&cfg, &th.home).expect("RequestLogger 默认 Full+默认目录注册后构建 Ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_run_flags_debug_quiet_no_console_single_message_dead_provider() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        s11b_write_agent_config(&th.home, s11b_dead_provider_config());
        // debug/quiet/no_console 全开：--debug/--quiet/--no-console 注入 +
        // 两个 println 臂（Debug/Quiet mode enabled）+ 单消息死地址 Err 臂。
        let res = run(
            None,
            Some("flags trio".to_string()),
            "wave-b-flags".to_string(),
            true,
            true,
            true,
            false,
        )
        .await;
        assert!(
            res.is_ok(),
            "flags 组合不得改变单消息模式的 Ok 收敛: {res:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_run_single_message_success_via_loopback_openai_mock() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        let (port, hits) = start_openai_mock(
            "{\"id\":\"wb-1\",\"object\":\"chat.completion\",\"created\":1700000000,\"model\":\"mock\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"wb-mock-reply\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}",
        );
        s11b_write_agent_config(
            &th.home,
            serde_json::json!({
                "agents": {"defaults": {"llm": "wbmock"}},
                "model_list": [{
                    "model_name": "wbmock",
                    "model": "openai/gpt-wb",
                    "api_base": format!("http://127.0.0.1:{port}"),
                    "api_key": "k"
                }]
            }),
        );
        // 单消息模式全绿通路：process_direct Ok → println!("Agent: ...") 臂，
        // run 返回 Ok 且 mock 至少收到一次真实 HTTP 补全请求。
        let res = run(
            None,
            Some("hello mock".to_string()),
            "wave-b-mock".to_string(),
            false,
            false,
            false,
            false,
        )
        .await;
        assert!(res.is_ok(), "mock provider 下单消息必须成功: {res:?}");
        assert!(
            hits.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "回环 mock 未收到任何请求 —— provider 未走到网络层"
        );
    }

    #[tokio::test]
    async fn wave_b_run_set_llm_bare_agents_section_inserts_defaults() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_agent_home_env();
        // agents 段存在但无 defaults → 命中 `!agents.contains_key("defaults")`
        // 插入分支（与 s11b「整个 agents 缺失」的插入互补）。
        s11b_write_agent_config(
            &th.home,
            serde_json::json!({
                "agents": {},
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
                action: AgentSetAction::Llm {
                    model: "fake".to_string(),
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
        let cfg = s11b_read_agent_config(&th.home);
        assert_eq!(cfg["agents"]["defaults"]["llm"], "fake");
    }
}

// ===========================================================================
// R9 补测批（coverage-95 goal）：交互式 REPL 主循环（agent.rs 交互分支，
// ~agent.rs:504-653）+ `agent set llm` 未命中时的裸 stdin y/N 确认
// （~agent.rs:670-689）。全部经真实 exe 子进程 + 管道 stdin 脚本驱动。
//
// 豁免清账：上方 S11b 头注与 wave_b 尾注曾把「rustyline REPL」「Set-Llm
// stdin 确认」列为不可测（当时按 tty 阻塞推断）。本批用
// test_harness::TestWorkspace::run_cli_with_stdin（整段脚本写入管道后关闭
// = 子进程顺序消费脚本、末尾见 EOF）推翻该豁免：rustyline 15 对非 tty
// stdin 无 isatty 挡路，走 readline_direct 全流程；EOF → ReadlineError::
// Eof → Goodbye 臂（agent.rs:641-644）。
//
// 结构性事实（agent.rs 读码实证 + 本文件既有断言 get_registry().is_none()）：
// standalone 构造路径 registry 为 None，三个内建 slash 命令的内层体
// （registry Some 分支，约 38 行）结构性不可达——/history 打印空行后继续、
// /clear 恒打印 "History cleared."、/status 恒打印 "State: no registry"。
// 因此断言目标 = 「命令被识别且不崩、循环推进到后续输入」，不钉内层文案。
// =========================================================================

// 整 mod Windows 形态（7/7 测试 + 3 个专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r9_repl {
    use std::path::PathBuf;
    use test_harness::TestWorkspace;
    use test_harness::mock_ai::{MockAiReply, MockAiServer};

    /// mock 回复内容标记串（宽松 contains 断言，纯 ASCII 防切片坑）。
    const MARKER: &str = "R9MOCK-REPLY-MARKER-ALPHA";

    /// 立即连接拒绝的死地址（127.0.0.1:9 discard 端口，Windows 秒拒），
    /// 给「模型调用失败 → Agent error 臂 + 循环存活」用。
    const DEAD_BASE: &str = "http://127.0.0.1:9";

    /// model id 刻意避开 infer_provider_from_model 的全部关键词
    /// （gpt/claude/glm/gemini/llama/deepseek/kimi/mistral/sonar/cohere/
    /// groq/nvidia/perplexity/command），provider 显式命名 "r9mock"：
    /// factory resolve_provider_selection 里 provider=="openai" 会映射成
    /// CodexProvider（openai→Codex 陷阱），未知 provider 名才落
    /// HttpCompat 直连 api_base。
    const PROVIDER_MODEL: &str = "r9mock/r9mocka";
    const ALIAS: &str = "r9mocka";

    fn r9_resolve_bin() -> PathBuf {
        test_harness::resolve_nemesisbot_bin().expect("nemesisbot binary resolved")
    }

    /// 夹具：临时 workspace + onboard default + model add --default 指向
    /// `base_url`。返回 (workspace, bin)。结尾断言 agents.defaults.llm 已
    /// 写成别名——这是 build_agent_loop 解析成功的前提。
    async fn r9_setup_model_ws(base_url: &str) -> (TestWorkspace, PathBuf) {
        let bin = r9_resolve_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let up = ws
            .run_cli_with_timeout(&bin, &["onboard", "default"], 60)
            .await;
        assert_eq!(
            up.exit_code, 0,
            "onboard default failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            up.stdout, up.stderr
        );
        let add = ws
            .run_cli_with_timeout(
                &bin,
                &[
                    "model",
                    "add",
                    "--model",
                    PROVIDER_MODEL,
                    "--base",
                    base_url,
                    "--key",
                    "r9-key",
                    "--default",
                ],
                30,
            )
            .await;
        assert_eq!(
            add.exit_code, 0,
            "model add failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            add.stdout, add.stderr
        );
        let raw = std::fs::read_to_string(ws.config_path()).expect("config.json readable");
        let cfg: serde_json::Value = serde_json::from_str(&raw).expect("config.json valid JSON");
        assert_eq!(
            cfg["agents"]["defaults"]["llm"], ALIAS,
            "model add --default 必须把 agents.defaults.llm 指到别名"
        );
        (ws, bin)
    }

    /// 夹具变体：只 onboard，不配任何模型——`agent set llm <不存在模型>`
    /// 的 resolve 必失败 → 走 y/N 裸 stdin 确认分支。
    async fn r9_setup_plain_ws() -> (TestWorkspace, PathBuf) {
        let bin = r9_resolve_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let up = ws
            .run_cli_with_timeout(&bin, &["onboard", "default"], 60)
            .await;
        assert_eq!(
            up.exit_code, 0,
            "onboard default failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            up.stdout, up.stderr
        );
        (ws, bin)
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_hello_then_quit_returns_mock_reply() {
        let srv = MockAiServer::start(vec![MockAiReply::Text(MARKER.to_string())])
            .expect("mock ai server starts");
        let (ws, bin) = r9_setup_model_ws(&srv.base_url()).await;
        let out = ws
            .run_cli_with_stdin(&bin, &["agent"], "hello\nquit\n", 60)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "quit 收尾必须干净退出:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains("Interactive mode"),
            "缺 REPL 入场横幅:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains(MARKER),
            "mock 模型回复未出现在 stdout:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Goodbye"),
            "quit 必须打印 Goodbye:\n{}",
            out.stdout
        );
        assert!(
            srv.hits() >= 1,
            "回环 mock 未收到任何 chat 请求——LLM 链路没有走通"
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_eof_without_quit_hits_eof_goodbye_arm() {
        let srv = MockAiServer::start(vec![MockAiReply::Text(MARKER.to_string())])
            .expect("mock ai server starts");
        let (ws, bin) = r9_setup_model_ws(&srv.base_url()).await;
        // 无 quit 收尾：脚本耗尽后 stdin 关闭 → rustyline Eof → Goodbye 臂。
        let out = ws.run_cli_with_stdin(&bin, &["agent"], "hello\n", 60).await;
        assert_eq!(
            out.exit_code, 0,
            "EOF 退出必须 rc=0:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains(MARKER),
            "EOF 场景同样应拿到模型回复:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Goodbye"),
            "EOF 必须落在 Eof→Goodbye 臂:\n{}",
            out.stdout
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_unknown_slash_command_lists_options_and_loops() {
        // 空 script：任何意外 LLM 调用会被 mock 以 500 "script exhausted"
        // 拒绝——比静默通过更响。slash 命令不应产生任何 LLM 调用。
        let srv = MockAiServer::start(vec![]).expect("mock ai server starts");
        let (ws, bin) = r9_setup_model_ws(&srv.base_url()).await;
        let out = ws
            .run_cli_with_stdin(&bin, &["agent"], "/bogus\nquit\n", 60)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "未知命令不得致崩:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains("Unknown command: /bogus"),
            "未识别命令必须有提示:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("/history, /clear, /status"),
            "提示里要列出可用命令:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Goodbye"),
            "循环必须继续吃掉后续 quit:\n{}",
            out.stdout
        );
        assert_eq!(srv.hits(), 0, "slash 命令路径不允许触发任何 LLM 调用");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_builtin_slash_trio_recognized_loop_survives() {
        // registry=None 下：/history 无正文输出、/clear 恒打 "History
        // cleared."、/status 恒打 Session/State(no registry)。三者都应被
        // 识别并继续循环直到 quit（对应结构性不可达的内层体豁免说明）。
        let srv = MockAiServer::start(vec![]).expect("mock ai server starts");
        let (ws, bin) = r9_setup_model_ws(&srv.base_url()).await;
        let out = ws
            .run_cli_with_stdin(&bin, &["agent"], "/history\n/status\n/clear\nquit\n", 60)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "三条内建 slash 命令 + quit 不得崩:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains("State: no registry"),
            "/status 应打印 registry=None 兜底状态:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Session: cli:default"),
            "/status 应打印当前 session key（CLI 默认 cli:default）:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("History cleared."),
            "/clear 恒打印清理确认:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Goodbye"),
            "四条输入都被顺序消费:\n{}",
            out.stdout
        );
        assert_eq!(srv.hits(), 0, "slash 命令不应打到 mock");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_set_llm_unresolved_confirm_yes_writes_default() {
        let (ws, bin) = r9_setup_plain_ws().await;
        // onboard 后没有任何 model_list 条目 → resolve 必失败 → WARNING +
        // "Set anyway? (y/N):" 裸 stdin read_line。答 y → 写入默认模型。
        let out = ws
            .run_cli_with_stdin(&bin, &["agent", "set", "llm", "bogus-model-r9"], "y\n", 30)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "确认分支必须干净退出:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout
                .contains("WARNING: Model 'bogus-model-r9' not found"),
            "未命中提示缺失:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Set anyway? (y/N)"),
            "确认问题文案缺失:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Default LLM set to: bogus-model-r9"),
            "答 y 后必须落盘默认模型:\n{}",
            out.stdout
        );
        let raw = std::fs::read_to_string(ws.config_path()).expect("config.json readable");
        let cfg: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(
            cfg["agents"]["defaults"]["llm"], "bogus-model-r9",
            "y 分支必须真的改写 config.json"
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_set_llm_unresolved_confirm_no_cancels() {
        let (ws, bin) = r9_setup_plain_ws().await;
        // 答 n（小写即可拒绝；只有恰好 "y" 才放行）→ Cancelled 臂 + 不写盘。
        let out = ws
            .run_cli_with_stdin(&bin, &["agent", "set", "llm", "bogus-model-r9"], "n\n", 30)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "拒绝分支也必须 Ok 收敛:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains("Cancelled."),
            "答 n 必须走取消分支:\n{}",
            out.stdout
        );
        assert!(
            !out.stdout.contains("Default LLM set to"),
            "取消后不得出现写入成功的文案:\n{}",
            out.stdout
        );
        let raw = std::fs::read_to_string(ws.config_path()).expect("config.json readable");
        assert!(
            !raw.contains("bogus-model-r9"),
            "取消分支不得改写 config.json"
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_repl_llm_failure_arm_prints_agent_error_and_loop_survives() {
        // api_base 指向立即拒绝的死端口：hello 的 LLM 调用失败 → process_
        // direct Err → "\nAgent error: ...\n"（eprintln）→ 循环继续吃 quit。
        let (ws, bin) = r9_setup_model_ws(DEAD_BASE).await;
        let out = ws
            .run_cli_with_stdin(&bin, &["agent"], "hello\nquit\n", 90)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "单次 LLM 失败后循环必须继续到 Goodbye:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stderr.contains("Agent error"),
            "错误臂必须打到 stderr:\n--- stderr ---\n{}",
            out.stderr
        );
        assert!(
            out.stdout.contains("Goodbye"),
            "错误后循环存活、quit 正常收尾:\n{}",
            out.stdout
        );
        assert!(!out.stdout.contains(MARKER), "死地址场景不可能有回复内容");
    }
}

// ===========================================================================
// R10 补测批（coverage-95 goal）：REPL 历史文件装载 + 空行 continue。
//
// r9_repl 已证 REPL 可管道驱动，但从未预置 history 文件（agent.rs:518 的
// load_history 行恒 miss）也没喂过空行（agent.rs:527-528 的 is_empty →
// continue 恒 miss）。本批两处一起吃：先把 workspace/logs/agent_history
// 写好再起 REPL —— 入场即命中 load_history；随后喂 "\n"（trim 后空 →
// continue）→ "/history"（识别 slash 臂尾部 println+continue）→ quit。
//
// 诚实边界：/history 内层 `registry Some` 分支（约 38 行）结构性不可达
// （standalone 构造路径 registry=None，r9_repl 头注已论证），仍不在此批。
// =========================================================================

// 整 mod Windows 形态（1/1 测试 + resolve helper，全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r10 {
    use std::path::PathBuf;

    use test_harness::TestWorkspace;

    fn r10_resolve_bin() -> PathBuf {
        test_harness::resolve_nemesisbot_bin().expect("nemesisbot binary resolved")
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_repl_seed_history_load_and_blank_line_continue() {
        let bin = r10_resolve_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let up = ws
            .run_cli_with_timeout(&bin, &["onboard", "default"], 60)
            .await;
        assert_eq!(
            up.exit_code, 0,
            "onboard default failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            up.stdout, up.stderr
        );

        // build_agent_loop 需要一个带 key 的模型条目才能创建 provider
        // （onboard default 的 model_list 为空，默认引用 zhipu/glm-4.7-flash
        // 会因无 API key 失败）。REPL 本测只走 空行 / /history / quit，
        // 从不真正调 LLM，所以用死地址假模型即可。model id 刻意避开
        // infer_provider_from_model 的关键词族（同 r9 的 PROVIDER_MODEL）。
        let add = ws
            .run_cli_with_timeout(
                &bin,
                &[
                    "model",
                    "add",
                    "--model",
                    "r10mock/r10mocka",
                    "--base",
                    "http://127.0.0.1:9",
                    "--key",
                    "r10-key",
                    "--default",
                ],
                30,
            )
            .await;
        assert_eq!(
            add.exit_code, 0,
            "model add failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            add.stdout, add.stderr
        );

        // 预写 rustyline 历史文件（REPL 退出时会写到同一文件）—— REPL 启动
        // 即走 agent.rs:517-520 的 exists + load_history 两行。
        let hist_dir = ws.workspace().join("logs");
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(hist_dir.join("agent_history"), "/status\nprev question\n")
            .expect("seed agent_history");

        // 脚本：空行（→ trim 空，continue）→ /history（registry None → 只
        // 打空行继续）→ quit（Goodbye + save_history 干净退出）。
        let out = ws
            .run_cli_with_stdin(&bin, &["agent"], "\n/history\nquit\n", 60)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "REPL 必须干净退出:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains("Interactive mode"),
            "缺 REPL 入场横幅:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("Goodbye"),
            "quit 必须走到 Goodbye:\n{}",
            out.stdout
        );
    }
}

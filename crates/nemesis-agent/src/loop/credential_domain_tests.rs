//! ②（2026-11-02 三期）：dispatch 凭据闸 v2——vault 别名域绑定 loop 级测试。
//!
//! 判定矩阵（设计；config.vault.json 路径 L26 修复 2026-11-11 起**双轨**：
//! 生产 = 装配点 `set_vault_domain_config_path` 注入 `<主workspace>/config/
//! config.vault.json`（CLI `vault bind` 写入侧同点）；本文件存量测试走
//! fallback 臂（`set_config_path` + config.json 同目录 sidecar——只服务
//! 未注入的测试/裸装配，生产行为不变）。文末两个 split_* 分裂锁用**两处
//! 独立目录**分别构造写读两侧，钉住「闸读注入路径、只读注入路径」）：
//! - 出域（绑定别名用在未授权 tool/slot）→ 工具不执行，拒绝串
//!   `layer:credential_domain` 回模型（audit 链条目只在 security plugin
//!   在场形态可达，本文件测无插件形态——拒绝串即审计面，诚实降级）；
//! - 域内 → 免卡直过（无插件环境下与 v1 跳过同形，可观察面 = 工具执行 +
//!   真值注入照常；域内/无绑定的分岔由 vault_domain 纯函数测试钉死）；
//! - 无绑定/配置缺席 → v1 路径（存量零回归）；
//! - 配置损坏 → fail-secure：warn 后按无绑定兜底，loop 不崩。

use super::credential_injection_tests::{MockLlmProvider, VAULT_SLOT_LOCK, tool_call_response};
use super::*;
use std::sync::{Arc, Mutex};

const SECRET: &str = "DOMAIN-TEST-SECRET-DO-NOT-LOG";

/// 本文件专用 config：工具名单可变（credtool / hdrtool 双注册）。
fn domain_test_config(tools: &[&str]) -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: tools.iter().map(|t| t.to_string()).collect(),
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 捕获 execute 实参的工具（credential 槽，字符串形态）。
struct CapturingCredTool {
    captured: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Tool for CapturingCredTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.captured.lock().unwrap().push(args.to_string());
        Ok("executed".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {"type": "string"},
                "credential": {"type": "string"}
            }
        })
    }
    fn credential_arg_keys(&self) -> &[&str] {
        &["credential"]
    }
}

/// 对象槽工具（headers 槽 = 对象，叶字段 `headers.<leaf>` 进闸面）。
struct HeaderCredTool {
    captured: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Tool for HeaderCredTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.captured.lock().unwrap().push(args.to_string());
        Ok("executed".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string"},
                "headers": {
                    "type": "object",
                    "properties": {"Authorization": {"type": "string"}}
                }
            }
        })
    }
    fn credential_arg_keys(&self) -> &[&str] {
        &["headers"]
    }
}

/// env 对象槽工具（exec 族形态，vault 计划 P3：`tool:env` 绑定域测试）。
struct EnvCredTool {
    captured: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Tool for EnvCredTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.captured.lock().unwrap().push(args.to_string());
        Ok("executed".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "env": {"type": "object"}
            }
        })
    }
    fn credential_arg_keys(&self) -> &[&str] {
        &["env"]
    }
}

/// 建 tempdir 工作面：config.json（tier 重载读的最小形态）+ 可选
/// config.vault.json；返回目录句柄（句柄活 = 目录活）。
fn setup_config_dir(vault_cfg: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), r#"{"agents": {}}"#).unwrap();
    if let Some(cfg) = vault_cfg {
        std::fs::write(dir.path().join("config.vault.json"), cfg).unwrap();
    }
    dir
}

fn done_response() -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: "done".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn resolver_for(alias: &str) {
    let alias = alias.to_string();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(move |a| {
        if a == alias {
            Ok(SECRET.to_string())
        } else {
            Err(format!("别名不存在: {a}"))
        }
    }));
}

/// 事件流拼接（ToolResult / Message / Error / Done 文本全量，供包含断言）。
fn events_text(events: &[AgentEvent]) -> String {
    let mut dump = String::new();
    for e in events {
        match e {
            AgentEvent::ToolResult(r) => dump.push_str(&r.result),
            AgentEvent::Message(m) | AgentEvent::Error(m) | AgentEvent::Done(m) => dump.push_str(m),
            AgentEvent::ToolCall(_) => {}
        }
        dump.push('\n');
    }
    dump
}

/// 出域：绑定别名用在未授权工具 → 拒绝串回模型、工具零执行。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn out_of_domain_rejects_without_execution() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    // corp 只绑给 web_fetch:headers —— credtool 上的使用即出域。
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert!(
        captured.lock().unwrap().is_empty(),
        "出域调用工具必须零执行"
    );
    let dump = events_text(&events);
    assert!(dump.contains("OUT OF DOMAIN"), "拒绝串必须可见: {dump}");
    assert!(
        dump.contains("layer:credential_domain"),
        "闸层标记必须可见: {dump}"
    );
    assert!(!dump.contains(SECRET), "拒绝面不得出现真值: {dump}");
}

/// 域内：绑定 (tool, slot) 精确命中 → 免卡直过，工具执行 + 真值注入照常。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn in_domain_passes_and_executes() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"credtool","slot":"credential"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "域内调用应执行一次");
    assert!(cap[0].contains(SECRET), "真值注入照常: {:?}", cap[0]);
    drop(cap);
    let dump = events_text(&events);
    assert!(!dump.contains("OUT OF DOMAIN"), "域内不得拒绝: {dump}");
}

/// 顶槽绑定覆盖叶字段：slot="headers" 盖住 headers.Authorization。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn top_slot_binding_covers_leaf_fields() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"hdrtool","slot":"headers"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response(
            "hdrtool",
            r#"{"url":"http://hook.example","headers":{"Authorization":"vault:corp"}}"#,
        ),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["hdrtool"]));
    agent_loop.register_tool(
        "hdrtool".to_string(),
        Box::new(HeaderCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["hdrtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "顶槽绑定应覆盖叶字段并执行");
    assert!(cap[0].contains(SECRET), "叶字段真值注入: {:?}", cap[0]);
    drop(cap);
    assert!(!events_text(&events).contains("OUT OF DOMAIN"));
}

/// 精确叶绑定不覆盖别的叶：slot="headers.Authorization" 只盖该叶，
/// 其他叶字段使用 = 出域拒绝。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn leaf_slot_mismatch_rejects() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"hdrtool","slot":"headers.Authorization"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response(
            "hdrtool",
            r#"{"url":"http://hook.example","headers":{"X-Api-Key":"vault:corp"}}"#,
        ),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["hdrtool"]));
    agent_loop.register_tool(
        "hdrtool".to_string(),
        Box::new(HeaderCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["hdrtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert!(captured.lock().unwrap().is_empty(), "叶错配必须零执行");
    assert!(events_text(&events).contains("OUT OF DOMAIN"));
}

/// 混合调用（一 ref 域内 + 一 ref 无绑定）→ NoBindings 优先级低于
/// OutOfDomain；此处两 ref 都域外/无绑定时落 v1 路径——headless（无审批
/// 通道）下 v1 = 跳过 = 执行（绑定 opt-in，存量零回归锚）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn unbound_alias_keeps_v1_path() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("free");
    // corp 有绑定但本次引用的是 free（无绑定）——refs 里 free 无绑定 →
    // NoBindings（corp 不在 refs，判定只看本次引用面）。
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:free"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "无绑定别名走 v1 路径（headless 放行）");
    assert!(cap[0].contains(SECRET));
    drop(cap);
    assert!(!events_text(&events).contains("OUT OF DOMAIN"));
}

/// 配置文件缺席 = 无绑定 = v1 行为（不 set_config_path 的既有测试形态的
/// 显式化——config_path 在场但 sidecar 缺席）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn missing_domain_config_behaves_as_unbound() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(None);

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert_eq!(captured.lock().unwrap().len(), 1, "缺席 = 无绑定 = v1 放行");
    assert!(!events_text(&events).contains("OUT OF DOMAIN"));
}

/// 损坏配置 fail-secure：warn 后按无绑定兜底——loop 不崩、工具照走 v1
/// 路径（headless 放行）；绑定绝不因坏配置放松任何调用的闸（有审批通道
/// 形态下坏配置回卡路径，此处无插件测不了卡面，分岔由 evaluate 纯函数
/// 测试钉死）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn malformed_domain_config_fails_secure() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some("{broken json"));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert_eq!(captured.lock().unwrap().len(), 1, "坏配置兜底 = v1 路径");
    assert!(!events_text(&events).contains("OUT OF DOMAIN"));
}

/// 出域拒绝无冷却（对抗清单 末行）：同键连续两次出域调用，每次
/// 确定性拒绝（判定臂无状态，v1 冷却属卡臂不属判定臂）；工具恒零执行。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn out_of_domain_rejects_deterministically_no_cooldown() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert!(captured.lock().unwrap().is_empty(), "重试仍出域 → 恒零执行");
    let dump = events_text(&events);
    assert_eq!(
        dump.matches("OUT OF DOMAIN").count(),
        2,
        "两次调用各出一条拒绝（无冷却静默）: {dump}"
    );
}

/// 出域拒绝的 audit 链条目（plugin 在场形态， 测试矩阵）：allow-all
/// 插件装配后管线放行、判定臂拒绝 → auditor 日志可查 denied +
/// policy_rule=security.credential_domain；值零参与（reason 无真值）。
#[cfg(feature = "security")]
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_domain_logs_audit_entry_with_plugin() {
    use nemesis_security::auditor::AuditFilter;
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    // allow-all 插件：8 层管线放行 credtool，闸拒绝只来自域判定臂。
    // JSONL 审计日志不在 plugin 构造期接线（生产由 security_setup.rs
    // set_audit_jsonl_log 激活）——测试同款显式接线；audit_chain_* 是
    // Merkle 链另一子系统，与 get_audit_log 读取面无关。
    let audit_dir = tempfile::tempdir().unwrap();
    let plugin = std::sync::Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        },
    ));
    plugin
        .auditor()
        .set_audit_jsonl_log(true, audit_dir.path().to_string_lossy().into_owned());
    let auditor = plugin.auditor();
    agent_loop.set_security_plugin(plugin);
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert!(captured.lock().unwrap().is_empty(), "出域必须零执行");
    assert!(events_text(&events).contains("OUT OF DOMAIN"));
    let entries = auditor.get_audit_log(AuditFilter {
        source: Some("credential_domain".to_string()),
        ..Default::default()
    });
    assert_eq!(entries.len(), 1, "恰一条域拒绝审计: {entries:?}");
    assert_eq!(entries[0].decision, "denied");
    assert_eq!(entries[0].policy_rule, "security.credential_domain");
    assert_eq!(entries[0].request.source, "credential_domain");
    let all = format!("{entries:?}");
    assert!(!all.contains(SECRET), "审计条目不得含真值: {all}");
}

// ---------------------------------------------------------------------------
// env 槽（exec 族，vault 计划 P3，2026-11-11）：`tool:env` 绑定形态
// ---------------------------------------------------------------------------

/// env 槽域内：绑定 (envtool, env) 精确命中 → 免卡直过，叶字段真值注入。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn env_slot_in_domain_passes_and_executes() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"envtool","slot":"env"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response(
            "envtool",
            r#"{"command":"printenv API_TOKEN","env":{"API_TOKEN":"vault:corp"}}"#,
        ),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["envtool"]));
    agent_loop.register_tool(
        "envtool".to_string(),
        Box::new(EnvCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["envtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "域内调用应执行一次");
    assert!(cap[0].contains(SECRET), "env 叶真值注入: {:?}", cap[0]);
    drop(cap);
    assert!(!events_text(&events).contains("OUT OF DOMAIN"));
}

/// env 槽出域：别名只绑 web_fetch:headers，用在 envtool:env → 拒绝 + 零执行。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn env_slot_out_of_domain_rejects() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    ));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response(
            "envtool",
            r#"{"command":"printenv API_TOKEN","env":{"API_TOKEN":"vault:corp"}}"#,
        ),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["envtool"]));
    agent_loop.register_tool(
        "envtool".to_string(),
        Box::new(EnvCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(dir.path().join("config.json"));
    let instance = AgentInstance::new(domain_test_config(&["envtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert!(captured.lock().unwrap().is_empty(), "出域必须零执行");
    let dump = events_text(&events);
    assert!(dump.contains("OUT OF DOMAIN"), "拒绝串必须可见: {dump}");
    assert!(!dump.contains(SECRET), "拒绝面不得出现真值: {dump}");
}

// ===== L26 路径分裂回归锁（2026-11-11，用户拍板案 A）=====
//
// BUG 形态：CLI `vault bind` 写 `<workspace>/config/config.vault.json`，
// 闸读 config.json 同目录——两侧各读各的，绑定生产永不生效。修复 = 闸
// primary 读装配点注入路径，fallback（config.json 同目录）只服务未注入
// 形态。headless 下「域内直过」与「无绑定走 v1」执行面同形不可区分，
// 所以分裂锁用**决定性形态**：两条路径各放相反映射，执行/拒绝的分叉
// 唯一由「闸读了哪条路径」决定。

/// 分裂锁 A：旧路径放**出域**映射 + 注入路径放**域内**映射 → 必须执行
/// 且注入真值（证明闸读注入路径；修复前闸读旧路径会 OUT OF DOMAIN 拒绝）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn split_gate_reads_injected_workspace_path() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    // 旧形态目录：config.json 旁的 config.vault.json = corp→web_fetch:headers
    //（对 credtool 是出域映射——闸若读这里必拒绝）。
    let legacy_dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    ));
    // 注入侧目录：生产布局 `<ws>/config/config.vault.json` = corp→credtool
    //（域内映射——闸读这里必放行）。
    let ws_dir = tempfile::tempdir().unwrap();
    let ws_cfg_dir = ws_dir.path().join("config");
    std::fs::create_dir(&ws_cfg_dir).unwrap();
    let injected_path = ws_cfg_dir.join("config.vault.json");
    std::fs::write(
        &injected_path,
        r#"{"bindings":{"corp":[{"tool":"credtool","slot":"credential"}]}}"#,
    )
    .unwrap();

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(legacy_dir.path().join("config.json"));
    agent_loop.set_vault_domain_config_path(injected_path);
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(
        cap.len(),
        1,
        "闸必须读注入路径（域内放行执行）；读旧路径=出域拒绝零执行"
    );
    assert!(cap[0].contains(SECRET), "真值注入照常: {:?}", cap[0]);
    drop(cap);
    assert!(
        !events_text(&events).contains("OUT OF DOMAIN"),
        "注入路径域内不得拒绝"
    );
}

/// 分裂锁 B（对偶）：注入路径放**出域**映射 + 旧路径放**域内**映射 →
/// 必须拒绝且零执行（证明注入路径优先且唯一生效；修复前/读旧路径都会
/// 经 v1 放行执行）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn split_injected_path_takes_precedence_over_legacy() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    // 旧形态目录：域内映射（闸若读这里会放行执行）。
    let legacy_dir = setup_config_dir(Some(
        r#"{"bindings":{"corp":[{"tool":"credtool","slot":"credential"}]}}"#,
    ));
    // 注入侧目录：出域映射（闸读这里必拒绝）。
    let ws_dir = tempfile::tempdir().unwrap();
    let ws_cfg_dir = ws_dir.path().join("config");
    std::fs::create_dir(&ws_cfg_dir).unwrap();
    let injected_path = ws_cfg_dir.join("config.vault.json");
    std::fs::write(
        &injected_path,
        r#"{"bindings":{"corp":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    )
    .unwrap();

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
        done_response(),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), domain_test_config(&["credtool"]));
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_config_path(legacy_dir.path().join("config.json"));
    agent_loop.set_vault_domain_config_path(injected_path);
    let instance = AgentInstance::new(domain_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert!(
        captured.lock().unwrap().is_empty(),
        "注入路径出域映射必须拒绝（读旧路径=域内放行执行）"
    );
    let dump = events_text(&events);
    assert!(dump.contains("OUT OF DOMAIN"), "拒绝串必须可见: {dump}");
    assert!(!dump.contains(SECRET), "拒绝面不得出现真值: {dump}");
}

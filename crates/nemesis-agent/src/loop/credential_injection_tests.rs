//! vault（2026-09-22 计划）：凭据别名注入测试。
//!
//! 两层：纯函数（改写语义）+ loop 级（四表面防泄漏——模型上下文/事件流
//! 里只有别名，真值只到达工具 execute）。

use super::credential_injection::inject_credential_aliases;
use super::credential_injection::{
    CredentialCardGuard, find_declared_vault_references, inject_credential_aliases_structured,
    inject_credential_aliases_with_secrets, try_card_recovery,
};
use super::*;
use nemesis_types::agent::{CredentialBroker, CredentialOutcome, CredentialRequest};
use parking_lot::Mutex as PlMutex;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 本文件专用 mock provider（s9_tests 同款：测试文件自包含）。
/// pub(super)：② 域绑定测试（credential_domain_tests）复用。
pub(super) struct MockLlmProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}
impl MockLlmProvider {
    pub(super) fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}
#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

pub(super) fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec!["credtool".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

// ---------------------------------------------------------------------------
// 纯函数：inject_credential_aliases
// ---------------------------------------------------------------------------

/// 无槽位声明的哑工具。
struct NoSlotTool;
#[async_trait]
impl Tool for NoSlotTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("x".to_string())
    }
}

/// 无槽位声明的工具：参数原样（机制零侵入）。
#[test]
fn no_slots_passthrough() {
    let tool = NoSlotTool;
    let args = r#"{"credential":"vault:alias","other":1}"#;
    assert_eq!(inject_credential_aliases(&tool, args).unwrap(), args);
}

/// 测试用带槽位工具。
struct SlotTool {
    keys: Vec<&'static str>,
}
impl SlotTool {
    fn new(keys: Vec<&'static str>) -> Self {
        Self { keys }
    }
}
#[async_trait]
impl Tool for SlotTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn credential_arg_keys(&self) -> &[&str] {
        &self.keys
    }
}

/// 全局解析器槽位是进程单例——**全 crate** 触及它的测试统一互斥在这把锁
/// （BUG-20261110-049）：此前 loop 内四个测试文件各持有独立 static 锁
/// （VAULT_SLOT_LOCK / 两个各自的 RESOLVER_LOCK / delegate_registry 无锁），
/// 互不互斥 = 假互斥，并行负载下 resolver 闭包被交叉覆写消费（断言收到
/// 别人测试的别名 / fs 类 setup 步骤偶发 panic 形态不一）。同 crate 新增
/// 触槽测试一律 `use` 这把锁，禁止再造文件级 static。
pub(crate) static VAULT_SLOT_LOCK: PlMutex<()> = PlMutex::new(());

/// 槽位 vault: 引用改写为真值；非槽位字段不动。
#[test]
fn slot_reference_rewritten() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "known" {
            Ok("REAL-SECRET".to_string())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let tool = SlotTool::new(vec!["credential"]);
    let out =
        inject_credential_aliases(&tool, r#"{"target":"http://x","credential":"vault:known"}"#)
            .unwrap();
    nemesis_config::clear_global_vault_resolver();
    assert!(out.contains("REAL-SECRET"), "真值应进 execute 参数: {out}");
    assert!(
        out.contains(r#""target":"http://x""#),
        "其它字段不动: {out}"
    );
}

/// 未知别名：Err 带补救指引（fail loud，工具不执行）。
#[test]
fn unknown_alias_fails_loud() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));
    let tool = SlotTool::new(vec!["credential"]);
    let err = inject_credential_aliases(&tool, r#"{"credential":"vault:ghost"}"#).unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    assert!(err.contains("CREDENTIAL REFERENCE FAILED"), "{err}");
    assert!(err.contains("vault set"), "应带补救指引: {err}");
}

/// 槽位是字面量（非 vault）：原样通过（向后兼容）。
#[test]
fn literal_slot_untouched() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::clear_global_vault_resolver();
    let tool = SlotTool::new(vec!["credential"]);
    let args = r#"{"credential":"plain-token"}"#;
    assert_eq!(inject_credential_aliases(&tool, args).unwrap(), args);
}

/// 非 JSON 参数：原样通过（交上游 schema 校验响亮失败）。
#[test]
fn malformed_json_passthrough() {
    let tool = SlotTool::new(vec!["credential"]);
    assert_eq!(
        inject_credential_aliases(&tool, "not-json").unwrap(),
        "not-json"
    );
}

// ---------------------------------------------------------------------------
// 对象槽递归（string→string 结构的字符串叶子改写，如 web_fetch headers）
// ---------------------------------------------------------------------------

/// 对象槽的 vault: 叶子改写为真值；非 vault 叶子与同层其它键不动。
#[test]
fn object_slot_leaves_rewritten() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "known" {
            Ok("REAL-SECRET".to_string())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let tool = SlotTool::new(vec!["headers"]);
    let out = inject_credential_aliases(
        &tool,
        r#"{"url":"http://x","headers":{"Authorization":"vault:known","X-Trace":"plain-val"}}"#,
    )
    .unwrap();
    nemesis_config::clear_global_vault_resolver();
    assert!(
        out.contains(r#""Authorization":"REAL-SECRET""#),
        "vault: 叶子应改写: {out}"
    );
    assert!(
        out.contains(r#""X-Trace":"plain-val""#),
        "非 vault 叶子不动: {out}"
    );
    assert!(out.contains(r#""url":"http://x""#), "槽外键不动: {out}");
}

/// 对象槽未知别名：fail loud，错误 field 带 `键.叶键` 定位。
#[test]
fn object_slot_unknown_leaf_fails_loud() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));
    let tool = SlotTool::new(vec!["headers"]);
    let err = inject_credential_aliases(&tool, r#"{"headers":{"Authorization":"vault:ghost"}}"#)
        .unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    assert!(err.contains("CREDENTIAL REFERENCE FAILED"), "{err}");
    assert!(
        err.contains("headers.Authorization"),
        "field 应带叶键定位: {err}"
    );
}

/// 生产声明端到端：WebFetchTool 自身声明 headers 槽——注入机制直接认。
#[test]
fn web_fetch_declares_headers_slot() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "known" {
            Ok("REAL-SECRET".to_string())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let tool = crate::loop_tools::WebFetchTool::new(1024);
    assert_eq!(tool.credential_arg_keys(), &["headers"]);
    let out = inject_credential_aliases(
        &tool,
        r#"{"url":"http://x","headers":{"Authorization":"vault:known"}}"#,
    )
    .unwrap();
    nemesis_config::clear_global_vault_resolver();
    assert!(out.contains("REAL-SECRET"), "生产工具声明应生效: {out}");
}

// ---------------------------------------------------------------------------
// env 槽（exec 族对象槽，vault 计划 P3，2026-11-11）：改写 + 明文清单收集
// ---------------------------------------------------------------------------

/// env 对象槽：`vault:` 叶子改写真值；`inject_credential_aliases_with_secrets`
/// （P2 掩码合流）带回解析明文清单——只收集 vault 引用，字面量叶不收集。
#[test]
fn env_slot_rewritten_and_secrets_collected() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| match alias {
        "known" => Ok("REAL-SECRET".to_string()),
        "known2" => Ok("REAL-SECRET-2".to_string()),
        _ => Err(format!("别名不存在: {alias}")),
    }));
    let tool = SlotTool::new(vec!["env"]);
    let (out, secrets) = inject_credential_aliases_with_secrets(
        &tool,
        r#"{"command":"printenv API_TOKEN","env":{"API_TOKEN":"vault:known","OTHER":"vault:known2","DEBUG":"1"}}"#,
    )
    .unwrap();
    nemesis_config::clear_global_vault_resolver();
    assert!(
        out.contains(r#""API_TOKEN":"REAL-SECRET""#),
        "vault 叶应改写: {out}"
    );
    assert!(
        out.contains(r#""OTHER":"REAL-SECRET-2""#),
        "第二个 vault 叶应改写: {out}"
    );
    assert!(out.contains(r#""DEBUG":"1""#), "字面量叶不动: {out}");
    assert!(
        out.contains(r#""command":"printenv API_TOKEN""#),
        "槽外键不动: {out}"
    );
    assert_eq!(secrets.len(), 2, "恰两个解析明文: {secrets:?}");
    assert!(secrets.contains(&"REAL-SECRET".to_string()), "{secrets:?}");
    assert!(
        secrets.contains(&"REAL-SECRET-2".to_string()),
        "{secrets:?}"
    );
}

/// env 槽未知别名：fail loud，field 带 `env.<叶键>` 定位（与 headers 同构）。
#[test]
fn env_slot_unknown_leaf_fails_loud() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));
    let tool = SlotTool::new(vec!["env"]);
    let err =
        inject_credential_aliases(&tool, r#"{"env":{"API_TOKEN":"vault:ghost"}}"#).unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    assert!(err.contains("CREDENTIAL REFERENCE FAILED"), "{err}");
    assert!(
        err.contains("env.API_TOKEN"),
        "field 应带 env 叶定位: {err}"
    );
}

/// env 槽全字面量：args 原样、明文清单为空——字面量本就在模型上下文里，
/// P2 收集语义只认 vault 引用（无新增泄漏面）。
#[test]
fn env_slot_literal_only_no_secrets_collected() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::clear_global_vault_resolver();
    let tool = SlotTool::new(vec!["env"]);
    let args = r#"{"command":"echo hi","env":{"DEBUG":"1","MODE":"x"}}"#;
    let (out, secrets) = inject_credential_aliases_with_secrets(&tool, args).unwrap();
    assert_eq!(out, args);
    assert!(secrets.is_empty(), "字面量不收集: {secrets:?}");
}

/// env 槽非 JSON 参数：透传（与注入层全局语义同构），零错误零清单。
#[test]
fn env_slot_malformed_json_passthrough() {
    let tool = SlotTool::new(vec!["env"]);
    let (out, secrets) = inject_credential_aliases_with_secrets(&tool, "not-json").unwrap();
    assert_eq!(out, "not-json");
    assert!(secrets.is_empty());
}

/// 生产声明 drift 锁（vault 计划 P3）：exec 族四工具（exec/run_checks/
/// run_script/background_start）必须声明 env 槽 + schema 带 env 参数
/// ——漏声明 = 注入/域闸/掩码三面对该工具全哑。
#[test]
fn exec_family_tools_declare_env_slot() {
    let exec = crate::loop_tools::ExecTool::new("/ws", false);
    let checks = crate::loop_tools::RunChecksTool::new("/ws");
    let script = crate::loop_tools::RunScriptTool::new("/ws", false);
    let registry =
        std::sync::Arc::new(crate::background_registry::BackgroundProcessRegistry::new());
    let bg = crate::background_registry::BackgroundStartTool::new("/ws", false, registry);
    for (name, tool) in [
        ("exec", &exec as &dyn Tool),
        ("run_checks", &checks),
        ("run_script", &script),
        ("background_start", &bg),
    ] {
        assert_eq!(tool.credential_arg_keys(), &["env"], "{name} 应声明 env 槽");
        let params = tool.parameters();
        assert!(
            params
                .get("properties")
                .and_then(|p| p.get("env"))
                .is_some(),
            "{name} schema 应带 env 参数: {params}"
        );
    }
}

// ---------------------------------------------------------------------------
// loop 级：通路 1 掩码合流（vault 计划 P2——注入明文在结果 Ok/Err 两臂遮蔽）
// ---------------------------------------------------------------------------

/// 事件流拼接（与 credential_domain_tests 同形——本文件自持一份）。
fn loop_events_text(events: &[AgentEvent]) -> String {
    let mut dump = String::new();
    for e in events {
        match e {
            AgentEvent::ToolCall(infos) => {
                for i in infos {
                    dump.push_str(&i.arguments);
                    dump.push('\n');
                }
            }
            AgentEvent::ToolResult(r) => dump.push_str(&r.result),
            AgentEvent::Message(m) | AgentEvent::Error(m) | AgentEvent::Done(m) => dump.push_str(m),
        }
        dump.push('\n');
    }
    dump
}

/// 回显实参的工具（捕获 + 结果回显 args——掩码测试的泄漏探针）。
struct EchoingCredTool {
    captured: Arc<Mutex<Vec<String>>>,
    fail: bool,
}
#[async_trait]
impl Tool for EchoingCredTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.captured.lock().unwrap().push(args.to_string());
        if self.fail {
            Err(format!("boom: {args}"))
        } else {
            Ok(format!("result: {args}"))
        }
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "credential": {"type": "string"}
            }
        })
    }
    fn credential_arg_keys(&self) -> &[&str] {
        &["credential"]
    }
}

/// Ok 臂：工具结果回显注入明文 → 掩码生效（模型只见 MASK_TEXT）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn loop_masks_vault_secret_in_ok_result() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "leak-test-alias" {
            Ok(SECRET.to_string())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:leak-test-alias"}"#),
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
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(EchoingCredTool {
            captured: captured.clone(),
            fail: false,
        }),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "工具应执行一次");
    assert!(cap[0].contains(SECRET), "execute 收到真值: {:?}", cap[0]);
    drop(cap);
    let dump = loop_events_text(&events);
    assert!(dump.contains("result:"), "结果应回灌: {dump}");
    assert!(!dump.contains(SECRET), "事件流不得含真值: {dump}");
    assert!(dump.contains(ALIAS_REF), "事件流只见别名引用: {dump}");
    assert!(
        dump.contains(crate::r#loop::session_secrets::MASK_TEXT),
        "结果掩码生效（P2 通路 1 合流）: {dump}"
    );
}

/// Err 臂：错误串回显注入明文 → 同样掩码（错误串也可能复述入参片段）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn loop_masks_vault_secret_in_err_result() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "leak-test-alias" {
            Ok(SECRET.to_string())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:leak-test-alias"}"#),
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
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(EchoingCredTool {
            captured: captured.clone(),
            fail: true,
        }),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    assert_eq!(captured.lock().unwrap().len(), 1);
    let dump = loop_events_text(&events);
    assert!(dump.contains("boom:"), "错误串应回灌: {dump}");
    assert!(!dump.contains(SECRET), "错误面不得含真值: {dump}");
    assert!(
        dump.contains(crate::r#loop::session_secrets::MASK_TEXT),
        "Err 臂掩码生效: {dump}"
    );
}

// ---------------------------------------------------------------------------
// loop 级：四表面防泄漏（真值只到 execute，事件流只见别名）
// ---------------------------------------------------------------------------

/// 捕获 execute 实参的工具（实现 credential_arg_keys）。
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

const SECRET: &str = "REAL-SECRET-VALUE-DO-NOT-LOG";
const ALIAS_REF: &str = "vault:leak-test-alias";

pub(super) fn tool_call_response(name: &str, args: &str) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_vault".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 端到端：provider 回 vault: 别名参数 → 工具收到真值；事件流（模型上下文
/// 的对映物）只见别名。会话历史 StoredToolCall / observer / request_logger
/// 均派生自同一 HookToolCall.arguments 字符串——事件流干净即四表面干净。
/// 全局 vault resolver 是进程级单槽（set/clear 全局函数），测试间必须
/// 串行——持锁跨 await 是刻意的：锁只在本测试 runtime 线程外排队别的
/// 测试，本 runtime 上无其他任务抢此锁（clippy 静态告警此处为误报）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn loop_injects_truth_but_events_carry_alias_only() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "leak-test-alias" {
            Ok(SECRET.to_string())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response(
            "credtool",
            r#"{"target":"http://hook.example","credential":"vault:leak-test-alias"}"#,
        ),
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
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    // 工具收到真值。
    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "工具应执行一次");
    assert!(
        cap[0].contains(SECRET),
        "execute 实参应含真值: {:?}",
        cap[0]
    );
    drop(cap);

    // 全部事件流无真值、有别名。
    let mut dump = String::new();
    for e in &events {
        match e {
            AgentEvent::ToolCall(infos) => {
                for i in infos {
                    dump.push_str(&i.arguments);
                    dump.push('\n');
                }
            }
            AgentEvent::ToolResult(r) => {
                dump.push_str(&r.result);
                dump.push('\n');
            }
            AgentEvent::Message(m) | AgentEvent::Error(m) | AgentEvent::Done(m) => {
                dump.push_str(m);
                dump.push('\n');
            }
        }
    }
    assert!(!dump.contains(SECRET), "事件流泄漏真值: {dump}");
    assert!(
        dump.contains(ALIAS_REF),
        "事件流应保留别名（透明度）: {dump}"
    );
}

/// 别名解析失败：工具不执行，错误串（无真值、带指引）作为工具结果回模型。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn loop_unknown_alias_blocks_execution() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = MockLlmProvider::new(vec![
        tool_call_response("credtool", r#"{"credential":"vault:ghost"}"#),
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
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    nemesis_config::clear_global_vault_resolver();

    // 工具未执行。
    assert!(
        captured.lock().unwrap().is_empty(),
        "注入失败时工具不得执行"
    );
    // 错误串带指引、进 ToolResult（模型可）。
    let tool_results: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult(r) => Some(r.result.clone()),
            _ => None,
        })
        .collect();
    assert!(
        tool_results
            .iter()
            .any(|r| r.contains("CREDENTIAL REFERENCE FAILED") && r.contains("vault set")),
        "工具结果应是带指引的错误: {tool_results:?}"
    );
}

// ---------------------------------------------------------------------------
// 结构化错误 + 发卡恢复 + 防滥用占位
// ---------------------------------------------------------------------------

/// 结构化错误：字段（field/alias/message）齐全，Display 与 legacy 错误串
/// 字节一致（模型可读契约零回归）。 oracle 改写生效后 message 为
/// 通用文案（无「别名不存在」marker、无别名回显）——存在性信号收敛。
#[test]
fn structured_error_display_matches_legacy_byte_exact() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));
    let tool = SlotTool::new(vec!["credential"]);
    let args = r#"{"credential":"vault:ghost"}"#;
    let legacy = inject_credential_aliases(&tool, args).unwrap_err();
    let structured = inject_credential_aliases_structured(&tool, args).unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    assert_eq!(structured.field, "credential");
    assert_eq!(
        structured.alias, "ghost",
        "alias 取自槽位值（L25 发卡不受改写影响）"
    );
    assert!(
        structured.message.contains("凭据引用无效"),
        "marker 根因应改写为通用文案: {}",
        structured.message
    );
    assert!(
        !structured.message.contains("别名不存在"),
        "通用文案不得含存在性 marker: {}",
        structured.message
    );
    assert!(
        !structured.message.contains("ghost"),
        "通用文案不得回显别名: {}",
        structured.message
    );
    assert_eq!(
        legacy,
        structured.to_string(),
        "Display 必须与 legacy 错误串字节一致"
    );
}

/// guard：占位/释放/TTL 过期/跨 session 与跨 alias 隔离。
#[test]
fn guard_acquire_release_ttl_and_isolation() {
    let g = CredentialCardGuard::new();
    let now = Instant::now();
    assert!(g.try_acquire_at("s1", "a1", now), "首次应可占位");
    assert!(
        !g.try_acquire_at("s1", "a1", now + Duration::from_secs(1)),
        "TTL 窗内不得再占"
    );
    assert!(g.try_acquire_at("s2", "a1", now), "跨 session 隔离");
    assert!(g.try_acquire_at("s1", "a2", now), "跨 alias 隔离");
    g.release("s1", "a1");
    assert!(g.try_acquire_at("s1", "a1", now), "release 后应可再占");
    // (s2, a1) 占位于 now：599s 仍占位、601s 已过期可再占。
    assert!(!g.try_acquire_at("s2", "a1", now + Duration::from_secs(599)));
    assert!(g.try_acquire_at("s2", "a1", now + Duration::from_secs(601)));
}

/// guard：空别名恒拒绝；硬顶拒绝 + 过期清扫恢复（内存上界契约）。
#[test]
fn guard_empty_alias_and_hard_cap() {
    let g = CredentialCardGuard::new();
    assert!(!g.try_acquire("s", ""), "空别名发不出卡");
    let now = Instant::now();
    for i in 0..4096 {
        assert!(g.try_acquire_at(&format!("s{i}"), "busy", now));
    }
    assert!(
        !g.try_acquire_at("overflow", "x", now),
        "硬顶（清扫后仍满）应拒绝发卡"
    );
    assert!(
        g.try_acquire_at("overflow", "x", now + Duration::from_secs(601)),
        "过期清扫后应恢复可卡"
    );
}

/// mock broker：返回指定裁决；可选把 key_name 写进共享集合（模拟 respond
/// 落 vault——解析器同集合现查，Stored 后即可解析）。
struct MockBroker {
    outcome: CredentialOutcome,
    store_into: Option<Arc<Mutex<std::collections::HashSet<String>>>>,
    calls: Arc<Mutex<usize>>,
}

impl CredentialBroker for MockBroker {
    fn request_credential(&self, request: &CredentialRequest) -> Result<CredentialOutcome, String> {
        *self.calls.lock().unwrap() += 1;
        // 卡载荷契约：request_id 非空、key_name=裸别名（无 vault: 前缀）、
        // reason 无值（此 mock 场景值只在解析器闭包里）。
        assert!(!request.request_id.is_empty());
        assert!(
            !request.key_name.starts_with("vault:"),
            "key_name 应为裸别名: {}",
            request.key_name
        );
        assert!(!request.reason.contains(SECRET), "卡载荷不得含值");
        if let Some(set) = &self.store_into {
            set.lock().unwrap().insert(request.key_name.clone());
        }
        Ok(self.outcome.clone())
    }
}

/// panic 探针 broker：被调即炸——证明 early-return 臂没碰 broker。
struct PanickingBroker;

impl CredentialBroker for PanickingBroker {
    fn request_credential(
        &self,
        _request: &CredentialRequest,
    ) -> Result<CredentialOutcome, String> {
        panic!("broker must not be called on this path");
    }
}

/// broker 缺席（headless/裸构建）：不发卡、不占位，None 回落原错误串。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn card_recovery_none_broker_returns_none() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));
    let tool = SlotTool::new(vec!["credential"]);
    let args = r#"{"credential":"vault:ghost"}"#;
    let err = inject_credential_aliases_structured(&tool, args).unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    let guard = CredentialCardGuard::new();
    let out = try_card_recovery(None, &guard, &tool, args, "s1", "credtool", &err).await;
    assert!(out.is_none(), "broker 缺席应走原错误路径");
    assert!(
        guard.try_acquire("s1", "ghost"),
        "broker 缺席不应占 guard 位"
    );
}

/// 发卡 → Stored → 重跑整 args 注入：真值进续行参数、卡只发一张、
/// guard 占位清除（立即可再卡）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn card_recovery_stored_reinjects_full_args() {
    let _g = VAULT_SLOT_LOCK.lock();
    let store = Arc::new(Mutex::new(std::collections::HashSet::new()));
    {
        let store = store.clone();
        nemesis_config::set_global_vault_resolver(std::sync::Arc::new(move |alias| {
            if store.lock().unwrap().contains(alias) {
                Ok(SECRET.to_string())
            } else {
                Err(format!("别名不存在: {alias}"))
            }
        }));
    }
    let tool = SlotTool::new(vec!["credential"]);
    let args = r#"{"target":"http://x","credential":"vault:card-alias"}"#;
    let err = inject_credential_aliases_structured(&tool, args).unwrap_err();
    assert_eq!(err.alias, "card-alias", "结构化 alias 应为裸别名");
    let calls = Arc::new(Mutex::new(0usize));
    let broker = Arc::new(MockBroker {
        outcome: CredentialOutcome::Stored,
        store_into: Some(store.clone()),
        calls: calls.clone(),
    });
    let guard = CredentialCardGuard::new();
    let out = try_card_recovery(Some(broker), &guard, &tool, args, "s1", "credtool", &err).await;
    nemesis_config::clear_global_vault_resolver();
    let (recovered, secrets) = out.expect("Stored 应恢复成功");
    assert!(
        recovered.contains(SECRET) && recovered.contains(r#""target":"http://x""#),
        "重注入应含真值且保留其余字段: {recovered}"
    );
    assert_eq!(
        secrets,
        vec![SECRET],
        "恢复臂应带回解析明文清单（掩码合流）"
    );
    assert_eq!(*calls.lock().unwrap(), 1, "应恰好发一张卡");
    assert!(
        guard.try_acquire("s1", "card-alias"),
        "Stored 应清占位（立即可再卡）"
    );
}

/// Denied：原错误串回落 + guard 占位保留（TTL 窗内不再发卡，过期恢复）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn card_recovery_denied_keeps_guard_occupancy() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Err(format!("别名不存在: {alias}"))
    }));
    let tool = SlotTool::new(vec!["credential"]);
    let args = r#"{"credential":"vault:denied-alias"}"#;
    let err = inject_credential_aliases_structured(&tool, args).unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    let calls = Arc::new(Mutex::new(0usize));
    let broker = Arc::new(MockBroker {
        outcome: CredentialOutcome::Denied,
        store_into: None,
        calls: calls.clone(),
    });
    let guard = CredentialCardGuard::new();
    let out = try_card_recovery(Some(broker), &guard, &tool, args, "s1", "credtool", &err).await;
    let t1 = Instant::now();
    assert!(out.is_none(), "Denied 应回落原错误串");
    assert_eq!(*calls.lock().unwrap(), 1);
    assert!(
        !guard.try_acquire_at("s1", "denied-alias", t1),
        "Denied 应保占位"
    );
    assert!(
        guard.try_acquire_at("s1", "denied-alias", t1 + Duration::from_secs(601)),
        "TTL 过期后应恢复可卡"
    );
}

/// 空 `vault:` 引用（alias 空）：不发卡（panic 探针 broker 证明没碰）。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn card_recovery_empty_alias_skips_broker() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::clear_global_vault_resolver();
    let tool = SlotTool::new(vec!["credential"]);
    let args = r#"{"credential":"vault:"}"#;
    let err = inject_credential_aliases_structured(&tool, args).unwrap_err();
    assert_eq!(err.alias, "", "空引用的结构化 alias 应为空");
    let guard = CredentialCardGuard::new();
    let out = try_card_recovery(
        Some(Arc::new(PanickingBroker)),
        &guard,
        &tool,
        args,
        "s1",
        "credtool",
        &err,
    )
    .await;
    assert!(out.is_none());
}

// ---------------------------------------------------------------------------
// oracle 收敛 + 闸面探测（find_declared_vault_references）
// ---------------------------------------------------------------------------

/// oracle 收敛：非 marker 根因（vault 已锁定）原文透传——环境级差分对任意
/// 别名同形，无逐别名存在性信号，模型可转述给用户做环境修复。
#[test]
fn oracle_non_marker_error_preserved_verbatim() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|_alias| {
        Err("vault 已锁定：argon2 模式需先 unlock".to_string())
    }));
    let tool = SlotTool::new(vec!["credential"]);
    let err =
        inject_credential_aliases_structured(&tool, r#"{"credential":"vault:any"}"#).unwrap_err();
    nemesis_config::clear_global_vault_resolver();
    assert_eq!(
        err.message, "vault 已锁定：argon2 模式需先 unlock",
        "非 marker 根因应原文透传"
    );
}

/// oracle 收敛：空 `vault:` 引用文案保留（无解析器安装的根因串不含
/// marker → 原样透传；alias 空 → 发卡不发）。
#[test]
fn oracle_empty_alias_message_preserved() {
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::clear_global_vault_resolver();
    let tool = SlotTool::new(vec!["credential"]);
    let err =
        inject_credential_aliases_structured(&tool, r#"{"credential":"vault:"}"#).unwrap_err();
    assert_eq!(err.alias, "");
    assert!(
        !err.message.contains("凭据引用无效"),
        "空引用根因不是「别名不存在」家族，不得被改写: {}",
        err.message
    );
    assert!(!err.message.is_empty(), "空引用应带根因串");
}

/// 闸面探测六形态：顶层 string 槽 / 对象叶（键.叶键 路径）/ 未声明槽里的
/// vault: 忽略（闸位精确性——不扩大到字面量明文管辖面）/ 非 JSON → 空 /
/// 槽缺席 → 空 / 空 alias 保留（空引用也是引用）。
#[test]
fn find_declared_vault_references_six_forms() {
    let tool = SlotTool::new(vec!["credential", "headers"]);

    // ① 顶层 string 槽。
    let refs = find_declared_vault_references(
        &tool,
        r#"{"credential":"vault:corp-key","target":"http://x"}"#,
    );
    assert_eq!(
        refs,
        vec![("credential".to_string(), "corp-key".to_string())]
    );

    // ② 对象叶：field 路径 = 键.叶键。
    let refs = find_declared_vault_references(
        &tool,
        r#"{"url":"http://x","headers":{"Authorization":"vault:corp-key","X-Trace":"plain"}}"#,
    );
    assert_eq!(
        refs,
        vec![("headers.Authorization".to_string(), "corp-key".to_string())]
    );

    // ③ 未声明槽里的 vault: 忽略（闸只看声明槽——精确性）。
    let refs = find_declared_vault_references(
        &tool,
        r#"{"undeclared":"vault:sneaky","target":"http://x"}"#,
    );
    assert!(refs.is_empty(), "未声明槽不得进闸面: {refs:?}");

    // ④ 非 JSON args → 空（与注入透传语义同构）。
    assert!(find_declared_vault_references(&tool, "not-json").is_empty());

    // ⑤ 声明槽缺席 → 空。
    assert!(find_declared_vault_references(&tool, r#"{"target":"http://x"}"#).is_empty());

    // ⑥ 空 alias 保留（`vault:` 前缀即引用，别名空也是引用形态）。
    let refs = find_declared_vault_references(&tool, r#"{"credential":"vault:"}"#);
    assert_eq!(refs, vec![("credential".to_string(), String::new())]);
}

/// 闸面探测：多别名一次调用全量收集（guard 合成键一次裁决罩整次调用的
/// 依据）；无槽位声明工具恒空（零新增摩擦）。
#[test]
fn find_declared_vault_references_multi_and_no_slots() {
    let tool = SlotTool::new(vec!["credential", "headers"]);
    let refs = find_declared_vault_references(
        &tool,
        r#"{"credential":"vault:a1","headers":{"Authorization":"vault:a2"}}"#,
    );
    assert_eq!(
        refs,
        vec![
            ("credential".to_string(), "a1".to_string()),
            ("headers.Authorization".to_string(), "a2".to_string()),
        ]
    );
    let bare = NoSlotTool;
    assert!(find_declared_vault_references(&bare, r#"{"credential":"vault:x"}"#).is_empty());
}

/// guard 合成键（闸冷却键 `gate:{tool}:{aliases}`）非空语义——绕开
/// guard 空别名恒 false 规则；同键占位/释放/TTL 与普通键同生命周期。
#[test]
fn guard_composite_gate_key_lifecycle() {
    let g = CredentialCardGuard::new();
    let key = "gate:web_fetch:corp-key";
    assert!(g.try_acquire("s1", key), "合成键恒非空应可占位");
    assert!(!g.try_acquire("s1", key), "窗内同键不得再占（冷却生效）");
    g.release("s1", key);
    assert!(g.try_acquire("s1", key), "批准 release 后同键应可再卡");
}

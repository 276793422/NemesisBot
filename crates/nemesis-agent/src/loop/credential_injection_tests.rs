//! P0 vault（C3，2026-09-22 计划 §3）：凭据别名注入测试。
//!
//! 两层：纯函数（改写语义）+ loop 级（四表面防泄漏——模型上下文/事件流
//! 里只有别名，真值只到达工具 execute）。

use super::credential_injection::inject_credential_aliases;
use super::*;
use parking_lot::Mutex as PlMutex;
use std::sync::Mutex;

/// 本文件专用 mock provider（s9_tests 同款：测试文件自包含）。
struct MockLlmProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}
impl MockLlmProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
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

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["credtool".to_string()],
        models: std::collections::HashMap::new(),
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

/// 全局解析器槽位是进程单例——触及它的测试互斥。
static VAULT_SLOT_LOCK: PlMutex<()> = PlMutex::new(());

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

/// 槽位是字面量（非 vault:）：原样通过（向后兼容）。
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

fn tool_call_response(name: &str, args: &str) -> LlmResponse {
    LlmResponse {
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
    // 错误串带指引、进 ToolResult（模型可见）。
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

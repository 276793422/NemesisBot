//! T2（内部专项总计划 L1 codemode，2026-10-13）— `code_run` 测试矩阵。
//!
//! 覆盖面：脚本执行/完成值四形态、脚本错误回传文本、脚本内 tool() 过完整
//! dispatch 瀑布（hooks 观测）+ 拒绝可 catch（estop / 安全管线 deny / 未知
//! 工具 / 递归闸 / loop 不可用）、资源闸（timeout 中断 / 内存上限）、输出
//! 截断、tier 双闸（声明 + mini 档 dispatch 拒绝）、args 校验、config 默认关。
//!
//! 全部 multi_thread flavor：execute → spawn_blocking → `Handle::block_on`
//! 桥需要 driver 在后台线程被驱动（current_thread 会挂死——与生产形态
//! 一致，见 code_run.rs 模块注释）。
#![cfg(all(feature = "codemode", not(target_os = "android")))]

use std::sync::{Arc, Mutex};

use super::*;
use crate::context::RequestContext;
use crate::estop::EstopState;
use crate::hooks::{HookToolCall, PostHookAction, ToolHook};
use crate::r#loop::Tool;
use crate::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
use crate::types::{AgentConfig, ToolCallInfo};

// —— 空 provider：不走 LLM ——

struct NoopProvider;

#[async_trait::async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

// —— echo 工具：脚本内调用的靶子 ——

struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn description(&self) -> String {
        "echoes its text argument".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "bad args".to_string())?;
        Ok(format!("echo:{}", v["text"].as_str().unwrap_or("")))
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn test_ctx() -> RequestContext {
    RequestContext::new("test", "chat1", "user1", "sess1")
}

/// echo 注册（&mut，须在 Arc 定型前）→ code_run 插件槽注册 → loop 注入。
fn build_loop() -> (Arc<AgentLoop>, Arc<CodeRunTool>) {
    let mut al = AgentLoop::new(Box::new(NoopProvider), test_config());
    al.register_tool("echo_tool".to_string(), Box::new(EchoTool));
    let al = Arc::new(al);
    let tool = Arc::new(CodeRunTool::new(10, 64));
    al.register_plugin_tool(CODE_RUN_TOOL_NAME.to_string(), tool.clone());
    tool.set_loop(Arc::downgrade(&al));
    (al, tool)
}

async fn run_script(tool: &CodeRunTool, script: &str) -> Result<String, String> {
    let args = serde_json::json!({ "script": script }).to_string();
    tool.execute(&args, &test_ctx()).await
}

async fn run_script_with(tool: &CodeRunTool, script: &str) -> String {
    run_script(tool, script)
        .await
        .expect("script must return text, not Err")
}

// ---------------------------------------------------------------------------
// 完成值四形态 + 脚本错误回传文本
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_value_number_string_undefined_object() {
    let (_al, tool) = build_loop();
    assert_eq!(
        run_script_with(&tool, "1+1").await,
        "2",
        "数字 → JSON 序列化"
    );
    assert_eq!(
        run_script_with(&tool, r#""hello""#).await,
        "hello",
        "字符串直取"
    );
    assert_eq!(
        run_script_with(&tool, "void 0").await,
        "(script completed with no return value)",
        "undefined → 诚实占位"
    );
    assert_eq!(
        run_script_with(&tool, "({a: 1, b: 'x'})").await,
        r#"{"a":1,"b":"x"}"#,
        "对象 → JSON 序列化"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_errors_return_as_text_not_err() {
    let (_al, tool) = build_loop();
    // throw 字符串。
    let out = run_script_with(&tool, r#"throw "boom";"#).await;
    assert!(out.starts_with("[code_run] 脚本错误:"), "{out}");
    assert!(out.contains("boom"), "异常 message 对模型可读: {out}");
    // 语法错误。
    let out = run_script_with(&tool, "this is not js )))").await;
    assert!(out.starts_with("[code_run] 脚本错误:"), "{out}");
    // 空参数 / 缺 script / 非法 JSON → 装配级 Err。
    assert!(tool.execute("{}", &test_ctx()).await.is_err());
    assert!(tool.execute("not json", &test_ctx()).await.is_err());
    assert!(
        tool.execute(r#"{"script": "  "}"#, &test_ctx())
            .await
            .is_err(),
        "空白脚本报错"
    );
}

// ---------------------------------------------------------------------------
// 脚本内 tool() → 完整 dispatch 瀑布
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_tool_call_goes_through_waterfall() {
    let (_al, tool) = build_loop();
    let out = run_script_with(&tool, r#"tool("echo_tool", JSON.stringify({text: "hi"}))"#).await;
    assert_eq!(out, "echo:hi");
}

/// hooks 观测：脚本内 tool() 的 dispatch 带 hooks（tool_event 观测面同源）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_tool_calls_fire_hooks() {
    struct SharedRecorder {
        hits: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait::async_trait]
    impl ToolHook for SharedRecorder {
        fn name(&self) -> String {
            "code_run-recorder".to_string()
        }
        async fn post_tool_use(&self, call: &HookToolCall, _result: &str) -> PostHookAction {
            self.hits.lock().unwrap().push(call.name.clone());
            PostHookAction::Continue
        }
    }

    let (al, tool) = build_loop();
    let shared: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    al.add_tool_hook(Arc::new(SharedRecorder {
        hits: shared.clone(),
    }));

    let out = run_script_with(
        &tool,
        r#"tool("echo_tool", JSON.stringify({text: "hooked"}))"#,
    )
    .await;
    assert_eq!(out, "echo:hooked");
    // 注：本断言只覆盖脚本内**嵌套**调用必经 hooks（echo_tool）；code_run
    // 自身的观测走 dispatch 路径（tier_dispatch_gate_rejects_mini 同款
    // handle_tool_call_at_depth 驱动）——本测试直接调 execute()，不经
    // dispatch，code_run 自身不该出现在 hooks 里。
    let seen = shared.lock().unwrap().clone();
    assert!(
        seen.iter().any(|n| n == "echo_tool"),
        "脚本内工具调用必经 hooks: {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// 拒绝面（可 catch、文案可观测）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_tool_throws_catchable() {
    let (_al, tool) = build_loop();
    let out = run_script_with(
        &tool,
        r#"
        try { tool("no_such_tool_xyz", "{}"); "no-throw" }
        catch (e) { "caught: " + e.message }
        "#,
    )
    .await;
    assert!(out.contains("caught:"), "{out}");
    assert!(out.contains("rejected"), "拒绝文案透传: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recursion_guard_blocks_code_run_self_call() {
    let (_al, tool) = build_loop();
    let out = run_script_with(
        &tool,
        r#"
        try { tool("code_run", "{}"); "no-throw" }
        catch (e) { "caught: " + e.message }
        "#,
    )
    .await;
    assert!(out.contains("cannot invoke itself"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_unavailable_is_honest() {
    // 未 set_loop：tool() 诚实报不可用。
    let orphan = CodeRunTool::new(10, 64);
    let out = run_script_with(
        &orphan,
        r#"
        try { tool("echo_tool", "{}"); "no-throw" }
        catch (e) { "caught: " + e.message }
        "#,
    )
    .await;
    assert!(out.contains("agent loop unavailable"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn estop_rejects_script_tool_calls() {
    let (al, tool) = build_loop();
    let estop = Arc::new(EstopState::new());
    al.set_estop(estop.clone());
    estop.trigger();

    let out = run_script_with(
        &tool,
        r#"
        try { tool("echo_tool", JSON.stringify({text: "x"})); "no-throw" }
        catch (e) { "caught: " + e.message }
        "#,
    )
    .await;
    assert!(out.contains("ESTOP"), "急停拒绝文案透传: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn security_deny_rejects_script_tool_calls() {
    let mut al = AgentLoop::new(Box::new(NoopProvider), test_config());
    al.register_tool("echo_tool".to_string(), Box::new(EchoTool));
    // default_action 默认 deny（SecurityPluginConfig::default）→ 一切拒绝。
    al.set_security_plugin(Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            ..Default::default()
        },
    )));
    let al = Arc::new(al);
    let tool = Arc::new(CodeRunTool::new(10, 64));
    al.register_plugin_tool(CODE_RUN_TOOL_NAME.to_string(), tool.clone());
    tool.set_loop(Arc::downgrade(&al));

    let out = run_script_with(
        &tool,
        r#"
        try { tool("echo_tool", JSON.stringify({text: "x"})); "no-throw" }
        catch (e) { "caught: " + e.message }
        "#,
    )
    .await;
    assert!(out.contains("SECURITY BLOCKED"), "安全管线拒绝透传: {out}");
    assert!(!out.contains("echo:"), "被拒调用不得执行: {out}");
}

// ---------------------------------------------------------------------------
// 资源闸
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_interrupts_infinite_loop() {
    let (_al, _tool) = build_loop();
    // 换 1s 预算的工具实例（build_loop 默认 10s——测试别等那么久）。
    let fast = Arc::new(CodeRunTool::new(1, 64));
    let out = run_script_with(&fast, "while (true) {}").await;
    assert!(
        out.starts_with("[code_run] 脚本错误:"),
        "死循环必须被中断: {out}"
    );
    assert!(out.len() < 200, "中断文案短小: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_limit_rejects_allocation_bomb() {
    let (_al, tool) = build_loop();
    // 64MB 堆：每次 ~8KB 的数组推 10 万次 ≈ 800MB 请求 → 快速 OOM。
    let out = run_script_with(
        &tool,
        r#"const a = []; for (let i = 0; i < 100000; i++) { a.push(new Array(1000).fill(1)); } "survived""#,
    )
    .await;
    assert!(
        out.starts_with("[code_run] 脚本错误:"),
        "分配炸弹必须被拒: {out}"
    );
    assert!(!out.contains("survived"), "不得跑完: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_output_is_truncated_with_note() {
    let (_al, tool) = build_loop();
    let out = run_script_with(&tool, r#""x".repeat(200000)"#).await;
    assert!(
        out.starts_with('x'),
        "截断保留前缀: {}",
        &out[..20.min(out.len())]
    );
    assert!(
        out.len() < MAX_OUTPUT_BYTES + 200,
        "截断上限: {}",
        out.len()
    );
    assert!(out.contains("[输出已截断"), "诚实注记在场");
}

// ---------------------------------------------------------------------------
// tier 双闸
// ---------------------------------------------------------------------------

#[test]
fn min_tier_is_big() {
    let tool = CodeRunTool::new(10, 64);
    assert_eq!(tool.min_tier(), Some("big"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tier_dispatch_gate_rejects_mini() {
    use nemesis_types::capability::ModelTier;

    let (al, _tool) = build_loop();
    al.set_tier(ModelTier::Mini);
    let call = ToolCallInfo {
        id: "t1".to_string(),
        name: CODE_RUN_TOOL_NAME.to_string(),
        arguments: r#"{"script": "1+1"}"#.to_string(),
    };
    let result = al.handle_tool_call_at_depth(&call, &test_ctx(), 0).await;
    assert!(
        result.contains("requires model tier 'big'"),
        "mini 档 dispatch 拒绝: {result}"
    );

    // big 档放行（闸在 execute 之前——拒绝不付 JS 引擎成本）。
    al.set_tier(ModelTier::Big);
    let result = al.handle_tool_call_at_depth(&call, &test_ctx(), 0).await;
    assert_eq!(result, "2", "big 档正常执行: {result}");
}

// ---------------------------------------------------------------------------
// config 默认关 + 资源默认值
// ---------------------------------------------------------------------------

#[test]
fn codemode_config_defaults_off() {
    let cfg = nemesis_config::CodeModeConfig::default();
    assert!(!cfg.enabled, "运行开关默认关");
    assert_eq!(cfg.timeout_secs, 30);
    assert_eq!(cfg.memory_limit_mb, 64);
    // 缺键反序列化 → 同一组默认。
    let parsed: nemesis_config::CodeModeConfig = serde_json::from_str("{}").unwrap();
    assert!(!parsed.enabled);
    assert_eq!(parsed.timeout_secs, 30);
    assert_eq!(parsed.memory_limit_mb, 64);
}

#[test]
fn tool_description_documents_budget() {
    let tool = CodeRunTool::new(30, 64);
    let d = tool.description();
    assert!(d.contains("30s"), "墙钟预算在文档里: {d}");
    assert!(d.contains("64MB"), "内存上限在文档里: {d}");
    assert!(d.contains("tool(name, argsJson)"), "调用面在文档里: {d}");
}

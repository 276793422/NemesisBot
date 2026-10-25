//! T2（内部专项总计划 L1 codemode，2026-10-13）— `code_run` 测试矩阵。
//!
//! 覆盖面：脚本执行/完成值四形态、脚本错误回传文本、脚本内 tool() 过完整
//! dispatch 瀑布（hooks 观测）+ 拒绝可 catch（estop / 安全管线 deny / 未知
//! 工具 / 递归闸 / loop 不可用）、资源闸（timeout 中断 / 内存上限）、输出
//! 截断、tier 双闸（声明 + mini 档 dispatch 拒绝）、args 校验、config 默认开
//!（2026-10-25 拍板）+ 开关热生效联动 + 预算超时文案（审批等待注记）。
//!
//! 全部 multi_thread flavor：execute → spawn_blocking → `Handle::block_on`
//! 桥需要 driver 在后台线程被驱动（current_thread 会挂死——与生产形态
//! 一致，见 code_run.rs 模块注释）。
#![cfg(all(feature = "codemode", not(target_os = "android")))]

use std::sync::{Arc, Mutex};

use super::imp::MAX_CONSOLE_LINES;
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
            finish_reason: None,
            escalated_to: None,
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
        tool_fence_profession: None,
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

/// P1-3（2026-10-25）：单次 tool() 吃穿剩余预算的超时路径——错误文案必须
/// 带审批等待注记（模型/用户可自诊断：审批卡最长 300s，预算耗在等待上时
/// 提示调 agents.codemode.timeout_secs）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_call_budget_timeout_carries_approval_note() {
    struct SlowTool;
    #[async_trait::async_trait]
    impl Tool for SlowTool {
        fn description(&self) -> String {
            "sleeps 3s then returns".to_string()
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            Ok("done".to_string())
        }
    }
    let mut al = AgentLoop::new(Box::new(NoopProvider), test_config());
    al.register_tool("slow_tool".to_string(), Box::new(SlowTool));
    let al = Arc::new(al);
    let tool = Arc::new(CodeRunTool::new(1, 64));
    al.register_plugin_tool(CODE_RUN_TOOL_NAME.to_string(), tool.clone());
    tool.set_loop(Arc::downgrade(&al));

    let out = run_script_with(&tool, r#"tool("slow_tool", "{}")"#).await;
    assert!(
        out.contains("exceeded the script wall-clock budget"),
        "预算超时文案: {out}"
    );
    assert!(
        out.contains("approval waits consume budget"),
        "审批等待注记: {out}"
    );
    assert!(
        out.contains("agents.codemode.timeout_secs"),
        "自诊断配置键: {out}"
    );
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
// config 默认开（2026-10-25 拍板）+ 资源默认值（timeout 300 = 审批卡对齐）
// ---------------------------------------------------------------------------

#[test]
fn codemode_config_defaults_on() {
    let cfg = nemesis_config::CodeModeConfig::default();
    assert!(cfg.enabled, "运行开关默认开");
    assert_eq!(cfg.timeout_secs, 300);
    assert_eq!(cfg.memory_limit_mb, 64);
    // 缺键反序列化 → 同一组默认。
    let parsed: nemesis_config::CodeModeConfig = serde_json::from_str("{}").unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.timeout_secs, 300);
    assert_eq!(parsed.memory_limit_mb, 64);
    // 用户显式 false 保持尊重（缺键才落默认）。
    let explicit_off: nemesis_config::CodeModeConfig =
        serde_json::from_str(r#"{"enabled": false}"#).unwrap();
    assert!(!explicit_off.enabled);
}

#[test]
fn tool_description_documents_budget() {
    let tool = CodeRunTool::new(300, 64);
    let d = tool.description();
    assert!(d.contains("300s"), "墙钟预算在文档里: {d}");
    assert!(d.contains("64MB"), "内存上限在文档里: {d}");
    assert!(d.contains("tool(name, argsJson)"), "调用面在文档里: {d}");
    assert!(d.contains("console"), "console 聚合面在文档里: {d}");
    assert!(d.contains("PREFERRED"), "批量编排引导语在文档里: {d}");
    assert!(
        d.contains("approval card consumes budget"),
        "审批等待吃预算的诚实注记在文档里: {d}"
    );
}

// ---------------------------------------------------------------------------
// 热生效联动（2026-10-25 默认开）：check_config_reload 比对注册状态增删
// ---------------------------------------------------------------------------

#[test]
fn config_reload_toggles_code_run_registration() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"agents": {"codemode": {"enabled": true}}}"#).unwrap();

    // Windows 的 last-write 时间来自系统 tick（粒度可达 15.6ms），两次紧邻
    // write 可能得到相同 mtime → check_config_reload 判「未变化」早退、
    // 开关切换丢失（Linux 纳秒精度从不撞，CI Windows shard 实锤过）。测试
    // 不赌墙钟：每次写入后显式 set_times 把 mtime 推到确定性递增的未来值。
    let write_cfg = |body: &str, ahead_ms: u64| {
        std::fs::write(&cfg_path, body).unwrap();
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&cfg_path)
            .unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(
            std::time::SystemTime::now() + std::time::Duration::from_millis(ahead_ms),
        ))
        .unwrap();
    };

    let al = Arc::new(AgentLoop::new(Box::new(NoopProvider), test_config()));
    let tool = Arc::new(CodeRunTool::new(300, 64));
    tool.set_loop(Arc::downgrade(&al));
    al.install_code_run_tool(tool.clone());
    // 装配期 enabled=true → 注册（agent_factory 同构）。
    al.register_plugin_tool(CODE_RUN_TOOL_NAME.to_string(), tool.clone());

    let has_code_run = || al.tool_names().iter().any(|n| n == CODE_RUN_TOOL_NAME);
    assert!(has_code_run(), "初始注册在场");

    // 建立基线 mtime（首次 check 记录，之后同 mtime 早退）。
    al.set_config_path(cfg_path.clone());
    al.check_config_reload();

    // 关 → 下一轮 check 移除（注册表成员资格切换，实例不重建）。
    write_cfg(r#"{"agents": {"codemode": {"enabled": false}}}"#, 10_000);
    al.check_config_reload();
    assert!(!has_code_run(), "enabled=false 后 code_run 必须移除");

    // 同 mtime 早退：再 check 仍是移除态。
    al.check_config_reload();
    assert!(!has_code_run());

    // 开 → 恢复。
    write_cfg(r#"{"agents": {"codemode": {"enabled": true}}}"#, 20_000);
    al.check_config_reload();
    assert!(has_code_run(), "enabled=true 后 code_run 必须恢复");

    // 已在册再 check（desired 不变）→ 不得产生 code_run_2（先查在册再注册）。
    al.check_config_reload();
    let names = al.tool_names();
    assert_eq!(
        names.iter().filter(|n| **n == CODE_RUN_TOOL_NAME).count(),
        1,
        "重复 check 不得产生 code_run_2: {names:?}"
    );
}

#[test]
fn config_reload_noop_when_tool_not_installed() {
    // 未装配（headless/项目 loop/集群 agent）→ sync 诚实缺席，不 panic、
    // 不自行注册。
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"agents": {"codemode": {"enabled": true}}}"#).unwrap();
    let al = Arc::new(AgentLoop::new(Box::new(NoopProvider), test_config()));
    al.set_config_path(cfg_path);
    al.check_config_reload();
    assert!(
        !al.tool_names().iter().any(|n| n == CODE_RUN_TOOL_NAME),
        "未装配时不得自行注册"
    );
}

#[test]
fn config_reload_missing_key_falls_back_to_enabled() {
    // `agents.codemode` 缺键 → 落 serde 默认（enabled=true）→ 在册保持。
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"agents": {}}"#).unwrap();

    let al = Arc::new(AgentLoop::new(Box::new(NoopProvider), test_config()));
    let tool = Arc::new(CodeRunTool::new(300, 64));
    tool.set_loop(Arc::downgrade(&al));
    al.install_code_run_tool(tool.clone());
    al.register_plugin_tool(CODE_RUN_TOOL_NAME.to_string(), tool);
    al.set_config_path(cfg_path);
    al.check_config_reload();
    assert!(
        al.tool_names().iter().any(|n| n == CODE_RUN_TOOL_NAME),
        "缺键落默认 enabled=true，在册保持"
    );
}

// ---------------------------------------------------------------------------
// console 聚合（A2 残余）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_log_aggregates_into_output() {
    let (_al, tool) = build_loop();
    let out = run_script_with(&tool, r#"console.log("hello", 42, {a: 1}); "done""#).await;
    assert!(out.contains("done"), "完成值仍在: {out}");
    assert!(out.contains("--- console output ---"), "聚合段标题: {out}");
    assert!(
        out.contains("hello 42 {\"a\":1}"),
        "字符串直取 + 数字/对象 JSON 序列化、空格 join: {out}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_warn_error_have_level_prefixes() {
    let (_al, tool) = build_loop();
    let out = run_script_with(
        &tool,
        r#"console.info("i"); console.warn("w"); console.error("e"); "ok""#,
    )
    .await;
    assert!(out.contains("\ni\n"), "info 与 log 同待遇（无前缀）: {out}");
    assert!(out.contains("[warn] w"), "warn 带级别前缀: {out}");
    assert!(out.contains("[error] e"), "error 带级别前缀: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_error_does_not_fail_script() {
    let (_al, tool) = build_loop();
    // console.error 是输出通道，不是脚本异常（脚本错误仍走 throw 路径）。
    let out = run_script_with(&tool, r#"console.error("boom-ish"); "fine""#).await;
    assert!(out.contains("fine"), "脚本照常成功: {out}");
    assert!(!out.contains("脚本错误"), "不得误报脚本错误: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_unused_adds_zero_bytes() {
    let (_al, tool) = build_loop();
    let out = run_script_with(&tool, "1+1").await;
    assert_eq!(out, "2", "不用 console → 输出零附加字节");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_line_cap_truncates_honestly() {
    let (_al, tool) = build_loop();
    let out = run_script_with(
        &tool,
        r#"for (let i = 0; i < 300; i++) { console.log("line-" + i); } "capped""#,
    )
    .await;
    let console_part = out.split("--- console output ---").nth(1).unwrap_or("");
    let logged = console_part
        .lines()
        .filter(|l| l.starts_with("line-"))
        .count();
    assert_eq!(logged, MAX_CONSOLE_LINES, "恰好 200 行入缓冲: {out}");
    assert!(
        console_part.contains("+ more console lines truncated at"),
        "截断诚实注记在场: {console_part}"
    );
    assert!(
        console_part.contains("line-199") && !console_part.contains("line-200\nline-"),
        "保留的是前 200 行（先到先得）: tail={:?}",
        console_part.lines().last()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_handles_edge_value_shapes() {
    let (_al, tool) = build_loop();
    let out = run_script_with(&tool, r#"console.log(undefined, null, [1,2]); "ok""#).await;
    assert!(
        out.contains("undefined null [1,2]"),
        "undefined 字面量 + null/数组 JSON 序列化: {out}"
    );
}

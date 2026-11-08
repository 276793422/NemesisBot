// workflow 工具节点 dispatch 入口五闸回归锁（A 批，非 security 面）。
//
// `handle_tool_call_with_attrib` 是 workflow tool 节点的程序化调度入口
//（gateway WorkflowSecureExecutor 消费）。本文件钉住该入口上与 security
// feature 无关的闸臂（闸序单一真相源 = dispatch 瀑布，workflow 路径不重
// 实现）：
//   ① estop 触发 → 拒绝（fail-closed，PLAN 回归锁 2）；
//   ② plan 模式写类节点拒绝 / build 放行（PLAN 回归锁 3）；
//   ③ hidden_tools 列表内工具拒绝（PLAN 回归锁 4）；
//   ④ 插件 min-tier 声明闸：低档拒 / 达档放行（PLAN 回归锁 5 的 dispatch
//      形态——tier_allowed_tools 白名单是供给侧选型闸，刻意不在本入口，
//      见 with_attrib docstring）；
//   ⑤ args_validator：Invalid 结构化拒绝 / Fixed 改写放行（PLAN 回归锁 6；
//      校验在 LLM 循环路径位于 dispatch 前，本入口收进自身）。
//
// 审批/审计归因面（PLAN 回归锁 7/9）依赖 security pipeline，在
// workflow_attrib_gate_tests.rs（cfg(security)）。
//
// fixture 自带（兄弟测试模块不能互相 import）。

use super::*;

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

struct WfGateProvider;

#[async_trait]
impl LlmProvider for WfGateProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
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

/// 固定 Ok 文本的工具。
struct WfOkTool(&'static str);

#[async_trait]
impl Tool for WfOkTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.0.to_string())
    }
}

/// 声明 min_tier 的工具（插件 min-tier dispatch 闸的消费形态）。
struct WfTierGateTool {
    text: &'static str,
    min_tier: &'static str,
}

#[async_trait]
impl Tool for WfTierGateTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.text.to_string())
    }
    fn min_tier(&self) -> Option<&str> {
        Some(self.min_tier)
    }
}

/// 记录收到的 args 的工具（Fixed 改写断言用；共享句柄在工具外断言）。
struct WfRecordingTool {
    last_args: std::sync::Arc<std::sync::Mutex<String>>,
}

#[async_trait]
impl Tool for WfRecordingTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        *self.last_args.lock().unwrap() = args.to_string();
        Ok("recorded_ok".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }
}

static WF_CALL_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn wf_call(name: &str, args: &str) -> ToolCallInfo {
    let n = WF_CALL_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    ToolCallInfo {
        id: format!("wf-gate-{n}"),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

/// workflow 形态的调用上下文（与 gateway WorkflowSecureExecutor 构造同构：
/// channel="workflow" + 触发会话三键）。
fn wf_ctx() -> RequestContext {
    RequestContext::new(
        "workflow",
        "chat-wf",
        "user-wf",
        "agent:main:session:wf-gate",
    )
}

fn wf_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(WfGateProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

// ---------------------------------------------------------------------------
// ① estop（PLAN 回归锁 2）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_estop_refuses_tool_node_call() {
    let mut agent_loop = wf_loop();
    let estop = std::sync::Arc::new(crate::estop::EstopState::new());
    estop.trigger();
    agent_loop.set_estop(estop);

    agent_loop.register_tool("wf_ok".to_string(), Box::new(WfOkTool("should_not_run")));
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("wf_ok", "{}"), &wf_ctx(), 0, &[])
        .await;
    assert!(
        out.contains("ESTOP"),
        "急停触发后 workflow tool 调用必须 fail-closed 拒绝: {out}"
    );
    assert!(
        !out.contains("should_not_run"),
        "急停态下工具体绝不能执行: {out}"
    );
}

// ---------------------------------------------------------------------------
// ② plan 模式写闸（PLAN 回归锁 3）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_plan_mode_refuses_write_and_build_allows() {
    let mut agent_loop = wf_loop();
    agent_loop.register_tool("write_file".to_string(), Box::new(WfOkTool("wrote_ok")));
    agent_loop.register_tool("read_file".to_string(), Box::new(WfOkTool("read_ok")));
    let ctx = wf_ctx();

    // Plan：写类节点拒绝（引导文案）。
    agent_loop.set_mode_with_event(crate::types::AgentMode::Plan, "", "");
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("write_file", r#"{"path":"a.txt"}"#), &ctx, 0, &[])
        .await;
    assert!(
        out.contains("Plan mode"),
        "plan 模式下 workflow 写类节点必须被拦: {out}"
    );
    assert!(out.contains("/build"), "拒绝文案必须引导切换 /build: {out}");
    // Plan：只读节点照常放行（写闸是名字面白名单，不是全拒）。
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("read_file", "{}"), &ctx, 0, &[])
        .await;
    assert_eq!(out, "read_ok", "plan 模式只读节点必须放行: {out}");

    // Build 切回：写类放行。
    agent_loop.set_mode_with_event(crate::types::AgentMode::Build, "", "");
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("write_file", r#"{"path":"a.txt"}"#), &ctx, 0, &[])
        .await;
    assert_eq!(out, "wrote_ok", "build 模式写类节点必须放行: {out}");
}

// ---------------------------------------------------------------------------
// ③ hidden_tools（PLAN 回归锁 4）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_hidden_tool_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    std::fs::write(&cfg, r#"{"agents": {"hidden_tools": ["wf_hidden_tool"]}}"#).unwrap();

    let mut agent_loop = wf_loop();
    agent_loop.set_config_path(cfg);
    agent_loop.register_tool(
        "wf_hidden_tool".to_string(),
        Box::new(WfOkTool("secret_ok")),
    );
    agent_loop.register_tool(
        "wf_visible_tool".to_string(),
        Box::new(WfOkTool("visible_ok")),
    );
    let ctx = wf_ctx();

    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("wf_hidden_tool", "{}"), &ctx, 0, &[])
        .await;
    assert!(
        out.contains("hidden by configuration"),
        "hidden 列表内工具经 workflow 调用必须拒绝: {out}"
    );
    assert!(!out.contains("secret_ok"), "被隐藏工具绝不能执行: {out}");

    // 对照：未隐藏工具照常执行（闸是按名单，不是全拒）。
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("wf_visible_tool", "{}"), &ctx, 0, &[])
        .await;
    assert_eq!(out, "visible_ok", "未隐藏工具必须照常执行: {out}");
}

// ---------------------------------------------------------------------------
// ④ 插件 min-tier 声明闸（PLAN 回归锁 5 的 dispatch 形态）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_min_tier_gate_low_tier_refused() {
    let mut agent_loop = wf_loop();
    agent_loop.register_tool(
        "wf_tiered_tool".to_string(),
        Box::new(WfTierGateTool {
            text: "tiered_ok",
            min_tier: "big",
        }),
    );
    let ctx = wf_ctx();

    // 低档（mini）→ 拒绝。
    agent_loop.set_tier(nemesis_types::capability::ModelTier::Mini);
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("wf_tiered_tool", "{}"), &ctx, 0, &[])
        .await;
    assert!(
        out.contains("requires model tier"),
        "低档模型下 min-tier 声明工具必须拒绝: {out}"
    );

    // 达档（big）→ 放行。
    agent_loop.set_tier(nemesis_types::capability::ModelTier::Big);
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("wf_tiered_tool", "{}"), &ctx, 0, &[])
        .await;
    assert_eq!(out, "tiered_ok", "达档模型必须放行: {out}");
}

// ---------------------------------------------------------------------------
// ⑤ args_validator（PLAN 回归锁 6）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_args_validator_invalid_and_autofix() {
    let mut agent_loop = wf_loop();
    let last_args = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    agent_loop.register_tool(
        "wf_schema_tool".to_string(),
        Box::new(WfRecordingTool {
            last_args: last_args.clone(),
        }),
    );
    let ctx = wf_ctx();

    // Invalid：缺必填字段 → 结构化校验错误（workflow 节点侧无模型自纠，
    // 诚实失败；文本带 Error: 前缀 → gateway 侧 tool_result_indicates_error
    // 映射 ToolResult::error 进 NodeResult 失败态）。
    let out = agent_loop
        .handle_tool_call_with_attrib(&wf_call("wf_schema_tool", "{}"), &ctx, 0, &[])
        .await;
    assert!(
        out.starts_with("Error: args validation failed"),
        "缺必填必须回结构化校验错误: {out}"
    );

    // Fixed：近邻 typo（pth→path 编辑距离 1）→ 自动改写后放行，
    // 且工具体收到的是改写后的 args（path 键在，pth 键不在）。
    let out = agent_loop
        .handle_tool_call_with_attrib(
            &wf_call("wf_schema_tool", r#"{"pth":"a.txt"}"#),
            &ctx,
            0,
            &[],
        )
        .await;
    assert_eq!(out, "recorded_ok", "近邻 typo 必须自动改写后放行: {out}");
    let seen = last_args.lock().unwrap().clone();
    let seen_json: serde_json::Value = serde_json::from_str(&seen)
        .unwrap_or_else(|e| panic!("工具收到的 args 必须是合法 JSON（{e}）: {seen}"));
    assert_eq!(
        seen_json.get("path").and_then(|v| v.as_str()),
        Some("a.txt"),
        "改写后工具必须收到 path 键: {seen}"
    );
    assert!(
        seen_json.get("pth").is_none(),
        "typo 键改写后必须消失: {seen}"
    );
}

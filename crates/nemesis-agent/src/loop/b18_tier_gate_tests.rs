// B18 工具缺失归因：内置工具 tier 白名单 dispatch 闸 + Unknown tool
// 轻归因测试。
//
// 覆盖面：
// ① `tier_supplies_tool` 单一真相源——白名单成员/非成员、Big 空表直通、
//    mcp_lazy meta 两件套豁免、min_tier 声明工具秩路径；
// ② dispatch 白名单闸——mini 硬调白名单外内置工具被拒（供给闸只滤
//    defs、registry 仍命中，不拦就直接执行的原始洞）；big 放行；
//    normal 档白名单成员放行、非成员拒绝；mcp_lazy 时 mcp_call 调用
//    豁免放行；min_tier 声明插件工具不重复拦（秩闸裁决后直通）；
//    workflow 工具节点（channel="workflow"）豁免白名单臂、min_tier
//    秩闸照常（模板点名工具非模型选型，与上游 workflow 入口语义对齐）；
// ③ Unknown tool 轻归因——`mcp_` 前缀指路 mcp_list、`plugin.` 前缀指路
//    插件安装面、其他名字裸文案（真未注册）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，min_tier_tests 同形）。

use super::*;

/// 恒定回复的迷你 provider。
struct B18MockProvider;

#[async_trait]
impl LlmProvider for B18MockProvider {
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

/// 无声明的哑工具（内置同形：min_tier=None）——执行即回固定标记。
struct B18PlainTool;

#[async_trait]
impl Tool for B18PlainTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("b18_plain_ok".to_string())
    }
}

/// 带 min_tier 声明的迷你插件工具（模拟 PluginToolBridge 声明面）。
struct B18TieredTool {
    min_tier: Option<String>,
}

#[async_trait]
impl Tool for B18TieredTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("b18_tiered_ok".to_string())
    }
    fn min_tier(&self) -> Option<&str> {
        self.min_tier.as_deref()
    }
}

fn b18_loop() -> AgentLoop {
    let cfg = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),

        tool_fence_profession: None,
    };
    AgentLoop::new(Box::new(B18MockProvider), cfg)
}

fn plain_call(name: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "call_b18".to_string(),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

// ---------------------------------------------------------------------------
// ① tier_supplies_tool 单一真相源
// ---------------------------------------------------------------------------

#[test]
fn b18_supplies_tool_whitelist_paths() {
    use nemesis_types::capability::ModelTier;
    // mini：白名单成员放行、非成员拒绝。
    assert!(tier_supplies_tool(None, "exec", ModelTier::Mini, false));
    assert!(!tier_supplies_tool(None, "spawn", ModelTier::Mini, false));
    assert!(!tier_supplies_tool(
        None,
        "question",
        ModelTier::Mini,
        false
    ));
    // normal：question 在列、spawn 不在列。
    assert!(tier_supplies_tool(
        None,
        "question",
        ModelTier::Normal,
        false
    ));
    assert!(!tier_supplies_tool(None, "spawn", ModelTier::Normal, false));
    // big/auto：空表 = 不过滤全放行。
    assert!(tier_supplies_tool(None, "spawn", ModelTier::Big, false));
    assert!(tier_supplies_tool(None, "anything", ModelTier::Auto, false));
    // mcp_lazy：meta 两件套豁免（mini 白名单本不含 mcp_call）。
    assert!(tier_supplies_tool(None, "mcp_call", ModelTier::Mini, true));
    assert!(tier_supplies_tool(None, "mcp_list", ModelTier::Mini, true));
    assert!(!tier_supplies_tool(
        None,
        "mcp_call",
        ModelTier::Mini,
        false
    ));
    // lazy 豁免只罩 meta 两件套，不罩 mcp_* 长尾。
    assert!(!tier_supplies_tool(
        None,
        "mcp_ghost",
        ModelTier::Mini,
        true
    ));
}

#[test]
fn b18_supplies_tool_min_tier_rank_path() {
    use nemesis_types::capability::ModelTier;
    // 声明工具走秩比较，不走白名单（mini 声明工具 mini 档放行——
    // 与白名单无关；big 声明工具 mini 档拒绝）。
    assert!(tier_supplies_tool(
        Some("mini"),
        "plugin.x.tool",
        ModelTier::Mini,
        false
    ));
    assert!(!tier_supplies_tool(
        Some("big"),
        "plugin.x.tool",
        ModelTier::Mini,
        false
    ));
    assert!(tier_supplies_tool(
        Some("big"),
        "plugin.x.tool",
        ModelTier::Big,
        false
    ));
}

// ---------------------------------------------------------------------------
// ② dispatch 白名单闸
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b18_dispatch_refuses_off_whitelist_builtin_at_mini() {
    let mut agent_loop = b18_loop();
    // spawn 是生产 registry 常驻工具（agent_factory 全量注册），mini 档
    // 供给面本无它（build_tool_defs tier 臂滤 defs）——B18 前硬调仍会
    // registry 命中直接执行（原始洞），现在被 dispatch 闸拒。
    agent_loop.register_tool("spawn".to_string(), Box::new(B18PlainTool));
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");

    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let out = agent_loop
        .handle_tool_call(&plain_call("spawn"), &ctx)
        .await;
    assert!(
        out.contains("not in the tool whitelist"),
        "mini 硬调白名单外内置工具必须被拒，实际: {out}"
    );
    assert!(
        out.contains("spawn") && out.contains("/model"),
        "拒绝文案必须带工具名 + 指路换档，实际: {out}"
    );

    // big 档同一调用放行执行（空白名单直通）。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Big;
    let out = agent_loop
        .handle_tool_call(&plain_call("spawn"), &ctx)
        .await;
    assert_eq!(out, "b18_plain_ok", "big 档同一调用必须执行，实际: {out}");
}

#[tokio::test]
async fn b18_dispatch_workflow_node_bypasses_whitelist_not_min_tier() {
    let mut agent_loop = b18_loop();
    agent_loop.register_tool("spawn".to_string(), Box::new(B18PlainTool));
    agent_loop.register_tool(
        "plugin.big.tool".to_string(),
        Box::new(B18TieredTool {
            min_tier: Some("big".to_string()),
        }),
    );
    // workflow 工具节点入口形态（生产 WorkflowSecureExecutor 同款）：
    // channel="workflow" 是程序化模板调用的判别子——模板作者显式点名
    // 工具，不受会话活跃档位白名单约束（上游 workflow_gate_tests ④
    // 钉死的语义，B18 闸接入共享瀑布后必须豁免白名单臂保住它）。
    let wf_ctx = RequestContext::new("workflow", "chat1", "session1", "sess1");

    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    // 白名单臂豁免：mini 档 workflow 节点调白名单外内置工具照常执行。
    let out = agent_loop
        .handle_tool_call(&plain_call("spawn"), &wf_ctx)
        .await;
    assert_eq!(
        out, "b18_plain_ok",
        "workflow 节点必须豁免白名单臂，实际: {out}"
    );

    // min_tier 臂不豁免：workflow 节点调 min_tier=big 插件工具仍被秩闸拒
    // （执行面能力下限对程序化调用一视同仁）。
    let out = agent_loop
        .handle_tool_call(&plain_call("plugin.big.tool"), &wf_ctx)
        .await;
    assert!(
        out.contains("requires model tier"),
        "workflow 节点必须照常执行 min_tier 秩闸，实际: {out}"
    );

    // 对照：同一 mini 档、普通通道同一工具仍被白名单臂拒（豁免只开
    // workflow 来源，聊天路径语义不变）。
    let web_ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let out = agent_loop
        .handle_tool_call(&plain_call("spawn"), &web_ctx)
        .await;
    assert!(
        out.contains("not in the tool whitelist"),
        "普通通道白名单语义必须不变，实际: {out}"
    );
}

#[tokio::test]
async fn b18_dispatch_normal_tier_boundary() {
    let mut agent_loop = b18_loop();
    agent_loop.register_tool("question".to_string(), Box::new(B18PlainTool));
    agent_loop.register_tool("spawn".to_string(), Box::new(B18PlainTool));
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");

    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Normal;
    // normal 白名单成员放行。
    let out = agent_loop
        .handle_tool_call(&plain_call("question"), &ctx)
        .await;
    assert_eq!(
        out, "b18_plain_ok",
        "normal 白名单成员必须执行，实际: {out}"
    );
    // 非成员拒绝（spawn 不在 normal 白名单）。
    let out = agent_loop
        .handle_tool_call(&plain_call("spawn"), &ctx)
        .await;
    assert!(
        out.contains("not in the tool whitelist"),
        "normal 白名单外内置工具必须被拒，实际: {out}"
    );
}

#[tokio::test]
async fn b18_dispatch_mcp_lazy_meta_exempt() {
    let agent_loop = b18_loop();
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    // mini + lazy on：mcp_call 过白名单闸，落到 mcp_call 语义校验
    // （内层 ghost 名的「不在已发现集合」文案——证明闸没拦）。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let cfg = tempfile::tempdir().unwrap();
    let path = cfg.path().join("config.json");
    std::fs::write(&path, r#"{"agents": {"mcp_lazy_supply": true}}"#).unwrap();
    agent_loop.set_config_path(path);
    let call = ToolCallInfo {
        id: "call_b18m".to_string(),
        name: "mcp_call".to_string(),
        arguments: r#"{"tool": "mcp_ghost"}"#.to_string(),
    };
    let out = agent_loop.handle_tool_call(&call, &ctx).await;
    assert!(
        out.contains("不在已发现集合") || out.contains("mcp"),
        "mcp_lazy 时 mcp_call 必须过白名单闸进入语义校验，实际: {out}"
    );
    assert!(
        !out.contains("not in the tool whitelist"),
        "mcp_call 不得被白名单闸拦（meta 两件套豁免），实际: {out}"
    );
}

#[tokio::test]
async fn b18_dispatch_declared_tool_not_double_blocked() {
    let mut agent_loop = b18_loop();
    // mini 声明插件工具：秩闸放行后白名单闸重判同一函数——幂等直通执行，
    // 不得因白名单不含 plugin.* 名被拦（供给闸绕白名单的秩语义 dispatch 端
    // 必须一致，防两臂失配）。
    agent_loop.register_tool(
        "plugin.mini.tool".to_string(),
        Box::new(B18TieredTool {
            min_tier: Some("mini".to_string()),
        }),
    );
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let out = agent_loop
        .handle_tool_call(&plain_call("plugin.mini.tool"), &ctx)
        .await;
    assert_eq!(
        out, "b18_tiered_ok",
        "声明工具不得被白名单闸重复拦，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// ③ Unknown tool 轻归因
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b18_unknown_tool_mcp_prefix_attribution() {
    let agent_loop = b18_loop();
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let out = agent_loop
        .handle_tool_call(&plain_call("mcp_ghost_tool"), &ctx)
        .await;
    assert!(out.contains("Unknown tool 'mcp_ghost_tool'"), "实际: {out}");
    assert!(
        out.contains("MCP") && out.contains("mcp_list"),
        "mcp_ 前缀必须归因 MCP 并指路 mcp_list，实际: {out}"
    );
}

#[tokio::test]
async fn b18_unknown_tool_plugin_prefix_attribution() {
    let agent_loop = b18_loop();
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let out = agent_loop
        .handle_tool_call(&plain_call("plugin.ghost.tool"), &ctx)
        .await;
    assert!(
        out.contains("Unknown tool 'plugin.ghost.tool'"),
        "实际: {out}"
    );
    assert!(
        out.contains("plugin") && (out.contains("installed") || out.contains("disabled")),
        "plugin. 前缀必须归因插件安装/停用面，实际: {out}"
    );
}

#[tokio::test]
async fn b18_unknown_tool_other_names_stay_bare() {
    let agent_loop = b18_loop();
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    // 非 mcp_/plugin. 名保持裸文案（真未注册）——f8/角色目录既有断言
    // 的契约不变。
    let out = agent_loop
        .handle_tool_call(&plain_call("no_such_tool"), &ctx)
        .await;
    assert_eq!(out, "Error: Unknown tool 'no_such_tool'", "实际: {out}");
}

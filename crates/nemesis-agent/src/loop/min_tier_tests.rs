// WASM 插件 min-tier 双闸测试（2026-09-30 插件体系复查 #2）。
//
// 覆盖面：
// ① 秩刻度——plugin_min_tier_rank / active_tier_rank 全枚举 + 兜底臂；
// ② 供给闸——声明 min_tier 的工具绕过 tier 白名单按秩供给（Big 档可见、
//    Mini 档按档收敛），无声明工具走原白名单路径（Mini 档照旧不可见）；
// ③ dispatch 闸——Mini 档调 big 档插件工具被拦（防缓存/竞态漏网），
//    Big 档同一工具放行执行；未注册工具文案不回归。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，f8_hidden_tools_tests 同形）。

use super::*;

/// 恒定回复的迷你 provider。
struct TierMockProvider;

#[async_trait]
impl LlmProvider for TierMockProvider {
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

/// 带 min_tier 声明的迷你插件工具（模拟 PluginToolBridge 的声明面）。
struct TieredPluginTool {
    min_tier: Option<String>,
}

#[async_trait]
impl Tool for TieredPluginTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("plugin_ok".to_string())
    }
    fn min_tier(&self) -> Option<&str> {
        self.min_tier.as_deref()
    }
}

fn tier_loop() -> AgentLoop {
    let cfg = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    AgentLoop::new(Box::new(TierMockProvider), cfg)
}

fn def_names(defs: &[crate::types::ToolDefinition]) -> Vec<String> {
    defs.iter().map(|d| d.function.name.clone()).collect()
}

// ---------------------------------------------------------------------------
// ① 秩刻度
// ---------------------------------------------------------------------------

#[test]
fn rank_scales_are_aligned() {
    use nemesis_types::capability::ModelTier;
    assert_eq!(plugin_min_tier_rank("mini"), 0);
    assert_eq!(plugin_min_tier_rank("normal"), 1);
    assert_eq!(plugin_min_tier_rank("big"), 2);
    // manifest 校验本就拦非法值；空串/垃圾按 big 兜底（最严）。
    assert_eq!(plugin_min_tier_rank(""), 2);
    assert_eq!(plugin_min_tier_rank("garbage"), 2);

    assert_eq!(active_tier_rank(ModelTier::Mini), 0);
    assert_eq!(active_tier_rank(ModelTier::Normal), 1);
    assert_eq!(active_tier_rank(ModelTier::Big), 2);
    // Auto 纯兜底（loop 内 tier 启动即 resolve）——按 big 最宽。
    assert_eq!(active_tier_rank(ModelTier::Auto), 2);
}

// ---------------------------------------------------------------------------
// ② 供给闸：声明档位绕白名单按秩供给
// ---------------------------------------------------------------------------

#[test]
fn supply_gate_filters_by_tier_rank() {
    let mut agent_loop = tier_loop();
    // big 档插件工具（manifest 缺省形态）。
    agent_loop.register_tool(
        "plugin.big.tool".to_string(),
        Box::new(TieredPluginTool {
            min_tier: Some("big".to_string()),
        }),
    );
    // mini 档插件工具（声明放宽——接线的意义所在）。
    agent_loop.register_tool(
        "plugin.mini.tool".to_string(),
        Box::new(TieredPluginTool {
            min_tier: Some("mini".to_string()),
        }),
    );
    // 无声明工具（内置同形：min_tier=None 走原白名单路径）。
    agent_loop.register_tool(
        "builtin_plain".to_string(),
        Box::new(TieredPluginTool { min_tier: None }),
    );

    // Big 档：三个全供给（空白名单直通 + 秩比较通过）。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Big;
    let names = def_names(&agent_loop.build_tool_defs());
    for expected in ["plugin.big.tool", "plugin.mini.tool", "builtin_plain"] {
        assert!(
            names.contains(&expected.to_string()),
            "Big 档 {expected} 必须在供给里，实际 {names:?}"
        );
    }

    // Mini 档：big 档插件工具被秩闸收敛；mini 档插件工具**绕白名单供给**
    // （白名单 13 个核心名不含 plugin.*，逐名收录不可扩展——这正是接线点）；
    // 无声明工具走原路径，不在 mini 白名单 → 不可见（与旧行为一致）。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"plugin.big.tool".to_string()),
        "Mini 档不得供给 big 档插件工具，实际 {names:?}"
    );
    assert!(
        names.contains(&"plugin.mini.tool".to_string()),
        "Mini 档必须供给 mini 档插件工具（绕白名单秩比较），实际 {names:?}"
    );
    assert!(
        !names.contains(&"builtin_plain".to_string()),
        "Mini 档无声明工具照旧走白名单（不在名单内），实际 {names:?}"
    );

    // Normal 档：big 档插件工具仍不可见，mini/normal 档均可见。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Normal;
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"plugin.big.tool".to_string())
            && names.contains(&"plugin.mini.tool".to_string()),
        "Normal 档收敛错误，实际 {names:?}"
    );
}

// ---------------------------------------------------------------------------
// ③ dispatch 闸：过档插件工具调用被拦
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_refuses_below_min_tier() {
    let mut agent_loop = tier_loop();
    agent_loop.register_tool(
        "plugin.big.tool".to_string(),
        Box::new(TieredPluginTool {
            min_tier: Some("big".to_string()),
        }),
    );
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "plugin.big.tool".to_string(),
        arguments: "{}".to_string(),
    };

    // Mini 档：供给面之外的第二道闸——陈旧 defs 快照也会被拦。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("requires model tier"),
        "Mini 档 dispatch 必须拦 big 档插件工具，实际: {out}"
    );
    assert!(
        out.contains("plugin.big.tool"),
        "拒绝消息必须带工具名，实际: {out}"
    );

    // Big 档：同一调用放行执行。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Big;
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert_eq!(out, "plugin_ok", "Big 档同一调用必须执行，实际: {out}");
}

#[tokio::test]
async fn dispatch_untouched_for_unregistered_and_undeclared() {
    let mut agent_loop = tier_loop();
    agent_loop.register_tool(
        "builtin_plain".to_string(),
        Box::new(TieredPluginTool { min_tier: None }),
    );
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");

    // 未注册工具不受影响，仍走原 Unknown tool 文案。
    let unknown = ToolCallInfo {
        id: "call_1".to_string(),
        name: "no_such_tool".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&unknown, &ctx).await;
    assert!(out.contains("Unknown tool"), "实际: {out}");

    // 无声明工具（min_tier=None）不进秩闸，Mini 档也照常执行。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let plain = ToolCallInfo {
        id: "call_2".to_string(),
        name: "builtin_plain".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&plain, &ctx).await;
    assert_eq!(out, "plugin_ok", "无声明工具不进秩闸，实际: {out}");
}

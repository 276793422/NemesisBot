// G1（2026-10-18 三方复评整改批三）：per-loop 附加闸槽测试。
//
// 覆盖面：
// ① `extra_hidden_tools` 合并语义——`current_hidden_tools` 单点合并（记录
//    tool_policy 折算面 ∪ config `agents.hidden_tools`），重叠去重、两源
//    各自生效；供给闸 `build_tool_defs` 对合并表生效；
// ② `tier_override` 钉死语义——`Some` = 记录档位唯一真相源
//    （`refresh_active_tier` 短路：config 重解析 / 换模型都不覆盖）；
//    `None` = 回落 config 驱动；档位供给闸随钉死生效（mini 白名单收窄）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，同 f8 形状）。

use super::*;

/// 恒定回复的迷你 provider。
struct G1MockProvider;

#[async_trait]
impl LlmProvider for G1MockProvider {
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

/// 迷你测试工具（执行成功返回固定文本）。
struct G1EchoTool;

#[async_trait]
impl Tool for G1EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("g1_echo_ok".to_string())
    }
}

fn g1_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(G1MockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: vec!["echo_alpha".to_string()],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

fn def_names(defs: &[crate::types::ToolDefinition]) -> Vec<String> {
    defs.iter().map(|d| d.function.name.clone()).collect()
}

fn write_config(dir: &tempfile::TempDir, agents_json: &str) -> std::path::PathBuf {
    let path = dir.path().join("config.json");
    std::fs::write(&path, format!(r#"{{"agents": {agents_json}}}"#)).unwrap();
    path
}

// ---------------------------------------------------------------------------
// ① extra_hidden_tools 合并语义
// ---------------------------------------------------------------------------

#[test]
fn extra_hidden_merges_with_config_source() {
    let agent_loop = g1_loop();
    let dir = tempfile::tempdir().unwrap();

    // 无 config 键：extra 单源生效；空表 = 清除。
    assert!(agent_loop.current_hidden_tools().is_empty());
    agent_loop.set_extra_hidden_tools(vec!["echo_beta".to_string()]);
    assert_eq!(agent_loop.current_hidden_tools(), vec!["echo_beta"]);
    agent_loop.set_extra_hidden_tools(Vec::new());
    assert!(agent_loop.current_hidden_tools().is_empty());

    // 两源并存：合并 + 重叠去重（config 项 trim 后撞 extra 已有项不重复入表）。
    let cfg = write_config(&dir, r#"{"hidden_tools": ["echo_alpha", "  ", ""]}"#);
    agent_loop.set_config_path(cfg);
    agent_loop.set_extra_hidden_tools(vec![
        "echo_beta".to_string(),
        "echo_alpha".to_string(),
        "mcp_*".to_string(),
    ]);
    let merged = agent_loop.current_hidden_tools();
    assert_eq!(
        merged,
        vec!["echo_beta", "echo_alpha", "mcp_*"],
        "config 的 echo_alpha 与 extra 重叠必须去重，实际 {merged:?}"
    );
}

#[tokio::test]
async fn build_tool_defs_filters_extra_hidden() {
    let mut agent_loop = g1_loop();
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(G1EchoTool));
    agent_loop.register_tool("echo_beta".to_string(), Box::new(G1EchoTool));
    agent_loop.register_tool("mcp_github".to_string(), Box::new(G1EchoTool));

    let dir = tempfile::tempdir().unwrap();
    // config 只藏 alpha；extra 藏 beta + mcp 通配 → 合并表三面全生效。
    let cfg = write_config(&dir, r#"{"hidden_tools": ["echo_alpha"]}"#);
    agent_loop.set_config_path(cfg);
    agent_loop.set_extra_hidden_tools(vec!["echo_beta".to_string(), "mcp_*".to_string()]);

    let names = def_names(&agent_loop.build_tool_defs());
    for hidden in ["echo_alpha", "echo_beta", "mcp_github"] {
        assert!(
            !names.contains(&hidden.to_string()),
            "合并表 {hidden} 必须被供给闸剔除，实际 {names:?}"
        );
    }
    // dispatch 闸同源：extra 藏的调用被拦（单一合并点双闸覆盖）。
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "echo_beta".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("hidden by configuration"),
        "dispatch 必须拦 extra 隐藏工具，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// ② tier_override 钉死语义
// ---------------------------------------------------------------------------

#[test]
fn tier_override_pins_against_config_refresh() {
    use nemesis_types::capability::ModelTier;
    let agent_loop = g1_loop();
    let dir = tempfile::tempdir().unwrap();

    // 基线：config 驱动——model_list 条目 model_tier=big → refresh 解析 Big。
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"model_list": [{"model": "test-model", "model_tier": "big"}]}"#,
    )
    .unwrap();
    agent_loop.set_config_path(cfg_path.clone());
    agent_loop.refresh_active_tier();
    assert_eq!(agent_loop.tier(), ModelTier::Big, "config 驱动基线");

    // 钉死 mini：立即生效 + refresh 短路（config 仍说 big，不被覆盖）。
    agent_loop.set_tier_override(Some(ModelTier::Mini));
    assert_eq!(agent_loop.tier(), ModelTier::Mini);
    agent_loop.refresh_active_tier();
    assert_eq!(agent_loop.tier(), ModelTier::Mini, "refresh 必须被钉死短路");

    // 换模型触发 refresh（set_provider_and_model 内部路径）——同样不覆盖。
    agent_loop.set_provider_and_model(Arc::new(G1MockProvider), "other-model".to_string());
    assert_eq!(
        agent_loop.tier(),
        ModelTier::Mini,
        "换模型不得覆盖记录级钉死"
    );

    // 解除：回落 config 驱动（active_model 已换 other-model，config 无该
    // 条目 → 名字启发式兜底；test-model 条目不再命中——重写 config 对准
    // 当前活跃模型验证回落读取链）。
    std::fs::write(
        &cfg_path,
        r#"{"model_list": [{"model": "other-model", "model_tier": "big"}]}"#,
    )
    .unwrap();
    agent_loop.set_tier_override(None);
    agent_loop.refresh_active_tier();
    assert_eq!(
        agent_loop.tier(),
        ModelTier::Big,
        "解除钉死后必须回 config 驱动"
    );
}

#[tokio::test]
async fn tier_override_narrows_tool_supply() {
    use nemesis_types::capability::ModelTier;
    let mut agent_loop = g1_loop();
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(G1EchoTool));
    agent_loop.register_tool("read_file".to_string(), Box::new(G1EchoTool));

    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, r#"{"defaults": {}}"#);
    agent_loop.set_config_path(cfg);

    // big（默认档，空白名单直通）：两工具都供给。
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(names.contains(&"echo_alpha".to_string()), "实际 {names:?}");

    // 钉死 mini：白名单收窄——read_file（核心 13）幸存，echo_alpha 出局。
    agent_loop.set_tier_override(Some(ModelTier::Mini));
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"echo_alpha".to_string()),
        "mini 档白名单必须收窄非核心工具，实际 {names:?}"
    );
    assert!(
        names.contains(&"read_file".to_string()),
        "mini 档核心工具必须幸存，实际 {names:?}"
    );
}

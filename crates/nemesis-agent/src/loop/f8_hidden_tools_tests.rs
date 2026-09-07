// F8 (devtool-upgrade 阶段 3)：`agents.hidden_tools` 双闸测试。
//
// 覆盖面（对应实施计划 F8 验收）：
// ① 通配匹配器——精确命中 / `mcp_*` 前缀通配 / 裸 `*` 全隐 / 不命中 / 空表；
// ② `current_hidden_tools` 新鲜读——无 config_path、缺键、坏 JSON → 空表
//    （不隐藏），合法键 → 解析+trim+跳过空项；
// ③ 供给闸——build_tool_defs 剔除被隐藏工具（精确+通配），其余照常供给；
// ④ dispatch 闸——已注册但被隐藏的工具调用被拦（防缓存/竞态漏网），
//    未注册工具仍走 "Unknown tool" 原文案；
// ⑤ 运行时翻转——不改进程重启，只改 config.json，下一拍供给与分发同步
//    解除隐藏（fresh-read 语义）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;

/// 恒定回复的迷你 provider（同 spawn_detached_tests 形状）。
struct HiddenMockProvider;

#[async_trait]
impl LlmProvider for HiddenMockProvider {
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

/// 迷你测试工具（执行成功返回固定文本）。
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("echo_ok".to_string())
    }
}

fn f8_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["echo_alpha".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn f8_loop() -> AgentLoop {
    AgentLoop::new(Box::new(HiddenMockProvider), f8_config())
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
// ① 通配匹配器
// ---------------------------------------------------------------------------

#[test]
fn matcher_exact_wildcard_and_miss() {
    let list: Vec<String> = vec!["exec".to_string(), "mcp_*".to_string()];
    // 精确命中。
    assert!(tool_name_matches_hidden(&list, "exec"));
    // 通配前缀命中。
    assert!(tool_name_matches_hidden(&list, "mcp_github_create_issue"));
    assert!(tool_name_matches_hidden(&list, "mcp_"));
    // 不命中。
    assert!(!tool_name_matches_hidden(&list, "executor"));
    assert!(!tool_name_matches_hidden(&list, "exec_async"));
    assert!(!tool_name_matches_hidden(&list, "mcpx_tool"));
    // 空表 / 垃圾项不命中。
    assert!(!tool_name_matches_hidden(&[], "exec"));
    assert!(!tool_name_matches_hidden(
        &[String::new(), "  ".to_string()],
        "exec"
    ));
}

#[test]
fn matcher_bare_star_hides_everything() {
    let list: Vec<String> = vec!["*".to_string()];
    assert!(tool_name_matches_hidden(&list, "exec"));
    assert!(tool_name_matches_hidden(&list, "anything"));
}

// ---------------------------------------------------------------------------
// ② current_hidden_tools 新鲜读
// ---------------------------------------------------------------------------

#[test]
fn current_hidden_tools_defaults_and_parses() {
    // 无 config_path（standalone loop）→ 空表。
    let agent_loop = f8_loop();
    assert!(agent_loop.current_hidden_tools().is_empty());

    let dir = tempfile::tempdir().unwrap();

    // 缺键 → 空表。
    let cfg = write_config(&dir, r#"{"defaults": {}}"#);
    agent_loop.set_config_path(cfg);
    assert!(agent_loop.current_hidden_tools().is_empty());

    // 坏 JSON → 空表（诚实降级为不隐藏，不 panic）。
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "not json at all").unwrap();
    agent_loop.set_config_path(bad);
    assert!(agent_loop.current_hidden_tools().is_empty());

    // 合法键 → 解析 + trim + 跳过空项。
    let cfg = write_config(
        &dir,
        r#"{"hidden_tools": ["exec", "  mcp_*  ", "", "web_fetch"]}"#,
    );
    agent_loop.set_config_path(cfg);
    let hidden = agent_loop.current_hidden_tools();
    assert_eq!(hidden, vec!["exec", "mcp_*", "web_fetch"]);
}

// ---------------------------------------------------------------------------
// ③ 供给闸：build_tool_defs 剔除被隐藏工具
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_tool_defs_hides_configured_tools() {
    let mut agent_loop = f8_loop();
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("echo_beta".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("mcp_github".to_string(), Box::new(EchoTool));

    let dir = tempfile::tempdir().unwrap();

    // 无隐藏键：全量供给（standalone loop tier=Big → 空白名单直通）。
    let cfg = write_config(&dir, r#"{"defaults": {}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    for expected in ["echo_alpha", "echo_beta", "mcp_github"] {
        assert!(
            names.contains(&expected.to_string()),
            "未隐藏时 {expected} 必须在供给里，实际 {names:?}"
        );
    }

    // 精确 + 通配：echo_alpha 与全部 mcp_* 消失，echo_beta 幸存。
    let cfg = write_config(&dir, r#"{"hidden_tools": ["echo_alpha", "mcp_*"]}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"echo_alpha".to_string()),
        "精确隐藏必须生效，实际 {names:?}"
    );
    assert!(
        !names.contains(&"mcp_github".to_string()),
        "通配隐藏必须生效，实际 {names:?}"
    );
    assert!(
        names.contains(&"echo_beta".to_string()),
        "未列名工具不得连坐，实际 {names:?}"
    );
}

// ---------------------------------------------------------------------------
// ④ dispatch 闸：已注册但被隐藏的调用被拦
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_refuses_hidden_tool() {
    let mut agent_loop = f8_loop();
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(EchoTool));

    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, r#"{"hidden_tools": ["echo_alpha"]}"#);
    agent_loop.set_config_path(cfg);

    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "echo_alpha".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("hidden by configuration"),
        "dispatch 必须拦截被隐藏工具（防缓存/竞态漏网），实际: {out}"
    );

    // 未注册工具不受影响，仍走原 Unknown tool 文案。
    let unknown = ToolCallInfo {
        id: "call_2".to_string(),
        name: "no_such_tool".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&unknown, &ctx).await;
    assert!(
        out.contains("Unknown tool"),
        "未注册工具文案不回归，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// ⑤ 运行时翻转：改 config.json 即时生效（无需重启）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_flip_unhides_without_restart() {
    let mut agent_loop = f8_loop();
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(EchoTool));

    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");

    // 阶段 1：隐藏 → 供给无 + dispatch 拒。
    std::fs::write(&cfg_path, r#"{"agents": {"hidden_tools": ["echo_*"]}}"#).unwrap();
    agent_loop.set_config_path(cfg_path.clone());
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "echo_alpha".to_string(),
        arguments: "{}".to_string(),
    };
    assert!(def_names(&agent_loop.build_tool_defs()).is_empty());
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(out.contains("hidden by configuration"), "实际: {out}");

    // 阶段 2：改盘解除隐藏 → 供给回来 + dispatch 放行执行。
    std::fs::write(&cfg_path, r#"{"agents": {"hidden_tools": []}}"#).unwrap();
    let names = def_names(&agent_loop.build_tool_defs());
    assert_eq!(names, vec!["echo_alpha".to_string()], "实际 {names:?}");
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert_eq!(out, "echo_ok", "解除隐藏后必须真正执行，实际: {out}");
}

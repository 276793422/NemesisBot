// J3 (devtool-upgrade 阶段 4)：MCP 客户端补齐的 agent 侧测试。
//
// 覆盖面（对应实施计划 J3 验收）：
// ① `registered_server_prefixes` 纯函数——前向推导已注册服务器前缀
//    （替代旧实现按第 3 个下划线反向切的 bug）：
//    - server 名含下划线（`my_server`）时旧实现 `underscores[2]` 会
//      panic（键恰好 2 个下划线越界）——回归锁；
//    - 键含多段下划线（`mcp_github_create_issue`）时旧实现切错段；
//    - 未注册（无任何 `mcp_` 键）的服务器不产出前缀；
// ② `register_mcp_tool` 同名冲突改名——第二个同名注册为 `{base}_2`、
//    第三个为 `{base}_3`，首个保持原名，注册表总数 3。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;

/// 恒定回复的迷你 provider（同 f8/spawn_detached 测试形状）。
struct McpReloadMockProvider;

#[async_trait]
impl LlmProvider for McpReloadMockProvider {
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

/// 迷你 MCP adapter 工具（definition 固定，execute 恒成功）。
struct FakeMcpTool {
    def: nemesis_mcp::adapter::ToolDefinition,
}

impl FakeMcpTool {
    fn named(name: &str) -> Self {
        Self {
            def: nemesis_mcp::adapter::ToolDefinition {
                name: name.to_string(),
                description: "fake mcp tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }
    }
}

#[async_trait]
impl nemesis_mcp::adapter::Tool for FakeMcpTool {
    fn definition(&self) -> &nemesis_mcp::adapter::ToolDefinition {
        &self.def
    }

    async fn execute(&self, _args: serde_json::Value) -> nemesis_mcp::adapter::ToolResult {
        nemesis_mcp::adapter::ToolResult::ok("fake_ok")
    }
}

// ---------------------------------------------------------------------------
// ① registered_server_prefixes 纯函数
// ---------------------------------------------------------------------------

#[test]
fn prefixes_server_name_with_underscore_two_segment_key() {
    // 回归锁（旧实现越界 panic）：server "my_server" → 前缀 "mcp_my_server_"，
    // 键 "mcp_my_server_search" 恰好 2 个下划线——旧代码 underscores[2]
    // 在守卫 len()>=2 下越界 panic；新实现前向推导无此问题。
    let prefixes = registered_server_prefixes(
        &["my_server".to_string()],
        &["mcp_my_server_search".to_string()],
    );
    assert_eq!(prefixes, vec!["mcp_my_server_".to_string()]);
}

#[test]
fn prefixes_cut_at_server_boundary_not_third_underscore() {
    // 回归锁（旧实现切错段）：键 "mcp_github_create_issue"（3 个下划线）
    // 旧代码切到第 3 个下划线得 "mcp_github_create_"——错误的"已注册前缀"，
    // 会把 server "github_create"（若存在）误判已注册。新实现按配置名推导。
    let prefixes = registered_server_prefixes(
        &["github".to_string()],
        &["mcp_github_create_issue".to_string()],
    );
    assert_eq!(prefixes, vec!["mcp_github_".to_string()]);
    assert!(!prefixes.iter().any(|p| p == "mcp_github_create_"));
}

#[test]
fn prefixes_exclude_unregistered_server() {
    // 配置里有但注册表无任何对应键的服务器不产出前缀（没有新工具可发现）。
    let prefixes = registered_server_prefixes(
        &["alpha".to_string(), "beta".to_string()],
        &["mcp_alpha_search".to_string()],
    );
    assert_eq!(prefixes, vec!["mcp_alpha_".to_string()]);
}

#[test]
fn prefixes_empty_inputs() {
    assert!(registered_server_prefixes(&[], &["mcp_a_x".to_string()]).is_empty());
    assert!(registered_server_prefixes(&["a".to_string()], &[]).is_empty());
}

// ---------------------------------------------------------------------------
// ② register_mcp_tool 同名冲突改名
// ---------------------------------------------------------------------------

fn mcp_reload_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(McpReloadMockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: Vec::new(),
            models: std::collections::HashMap::new(),
        },
    )
}

#[test]
fn register_mcp_tool_renames_duplicates() {
    let l = mcp_reload_loop();
    l.register_mcp_tool(Box::new(FakeMcpTool::named("mcp_github_search")));
    l.register_mcp_tool(Box::new(FakeMcpTool::named("mcp_github_search")));
    l.register_mcp_tool(Box::new(FakeMcpTool::named("mcp_github_search")));

    let keys = l.tools.read().keys().cloned().collect::<Vec<String>>();
    assert_eq!(keys.len(), 3);
    for expected in [
        "mcp_github_search",
        "mcp_github_search_2",
        "mcp_github_search_3",
    ] {
        assert!(
            keys.iter().any(|k| k == expected),
            "missing '{expected}' in {keys:?}"
        );
    }
}

#[test]
fn register_mcp_tool_first_registration_keeps_base_name() {
    let l = mcp_reload_loop();
    l.register_mcp_tool(Box::new(FakeMcpTool::named("mcp_alpha_fetch")));
    let keys = l.tools.read().keys().cloned().collect::<Vec<String>>();
    assert_eq!(keys, vec!["mcp_alpha_fetch".to_string()]);
}

#[test]
fn register_mcp_tool_rename_skips_existing_suffixes() {
    // 预置一个手工键 "mcp_alpha_fetch_2"（模拟既有注册），
    // 第二次注册同名应跳到 "_3" 而非撞上已存在的 "_2"。
    let l = mcp_reload_loop();
    l.tools
        .write()
        .insert("mcp_alpha_fetch_2".to_string(), Arc::new(EchoToolSlot));
    l.register_mcp_tool(Box::new(FakeMcpTool::named("mcp_alpha_fetch")));
    l.register_mcp_tool(Box::new(FakeMcpTool::named("mcp_alpha_fetch")));

    let keys = l.tools.read().keys().cloned().collect::<Vec<String>>();
    assert!(keys.iter().any(|k| k == "mcp_alpha_fetch"));
    assert!(keys.iter().any(|k| k == "mcp_alpha_fetch_3"));
    assert_eq!(keys.len(), 3);
}

/// 占位最小工具（仅用于预置注册表键，永不被执行）。
struct EchoToolSlot;

#[async_trait]
impl Tool for EchoToolSlot {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("slot".to_string())
    }
}

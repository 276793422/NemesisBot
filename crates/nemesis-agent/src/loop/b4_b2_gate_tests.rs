// B4 + B2（2026-10-19 connector 边界梳理）：总开关双闸 + 独占接管闸测试。
//
// 覆盖面：
// ① `current_connectors_enabled` fresh-read——无 config_path / 缺键 / 坏
//    JSON / 读失败 → true（缺省开，「装了即生效」不因读失败翻转），
//    `connectors.enabled=false` → false；
// ② 供给闸（build_tool_defs）——总开关关闭时 connector_find/connector_invoke
//    不上供给面，其余工具不连坐；改盘即恢复（fresh-read）；
// ③ dispatch 闸（B4）——总开关关闭时两工具被诚实拒绝（防 prompt cache
//    陈旧带出/在途请求漏网），位置在 security 之前（拒绝不付 judge 成本
//    由闸序保证，测试钉住拒绝发生）；
// ④ 独占闸（B2 dispatch 端）——出处感知：非 `connector-binding` 通道直调
//    独占表内工具被拦；绑定回灌通道放行（治理正向路由，拒了它连接器
//    自己先瘫）；表外工具任何通道不拦；
// ⑤ 供给器注入——set_exclusive_hidden_provider 后 current_exclusive_hidden_tools
//    现算返回闭包输出；缺省（未注入，项目 loop / 独立装配形态）= 空表；
// ⑥ B4×B2 交叉（层级闸）——总开关关 = 独占闸一并释放：被独占的原生工具
//    恢复供给 + 直调恢复执行；总开关重开 = 独占接管恢复（回归锁：此前
//    独占表无 master 短路，总闸关闭后能力落入「治理面没了 + 原生直调被
//    拦」的砖死态）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;

/// 恒定回复的迷你 provider（同 f8_hidden_tools_tests 形状）。
struct GateMockProvider;

#[async_trait]
impl LlmProvider for GateMockProvider {
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
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("echo_ok".to_string())
    }
}

fn gate_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["echo_alpha".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn gate_loop() -> AgentLoop {
    AgentLoop::new(Box::new(GateMockProvider), gate_config())
}

fn def_names(defs: &[crate::types::ToolDefinition]) -> Vec<String> {
    defs.iter().map(|d| d.function.name.clone()).collect()
}

/// 写整份 config.json（body 为完整 JSON 对象文本）。
fn write_config(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let path = dir.path().join("config.json");
    std::fs::write(&path, body).unwrap();
    path
}

// ---------------------------------------------------------------------------
// ① current_connectors_enabled fresh-read
// ---------------------------------------------------------------------------

#[test]
fn connectors_enabled_defaults_and_parses() {
    // 无 config_path（standalone loop）→ 缺省开。
    let agent_loop = gate_loop();
    assert!(agent_loop.current_connectors_enabled());

    let dir = tempfile::tempdir().unwrap();

    // 缺 connectors 键 → 开。
    let cfg = write_config(&dir, r#"{"agents": {}}"#);
    agent_loop.set_config_path(cfg);
    assert!(agent_loop.current_connectors_enabled());

    // connectors 段存在但无 enabled 字段 → 开。
    let cfg = write_config(&dir, r#"{"connectors": {}}"#);
    agent_loop.set_config_path(cfg);
    assert!(agent_loop.current_connectors_enabled());

    // 坏 JSON → 开（诚实降级，不因读失败翻转默认态）。
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "not json at all").unwrap();
    agent_loop.set_config_path(bad);
    assert!(agent_loop.current_connectors_enabled());

    // 显式关闭。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": false}}"#);
    agent_loop.set_config_path(cfg);
    assert!(!agent_loop.current_connectors_enabled());
}

// ---------------------------------------------------------------------------
// ② 供给闸：build_tool_defs 剔除 connector_find/connector_invoke
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_tool_defs_filters_connector_tools_when_master_off() {
    let mut agent_loop = gate_loop();
    agent_loop.register_tool("connector_find".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("connector_invoke".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(EchoTool));

    let dir = tempfile::tempdir().unwrap();

    // 总开关缺省开：三工具全在供给面。
    let cfg = write_config(&dir, r#"{"agents": {}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    for expected in ["connector_find", "connector_invoke", "echo_alpha"] {
        assert!(
            names.contains(&expected.to_string()),
            "总开关开时 {expected} 必须在供给里，实际 {names:?}"
        );
    }

    // 总开关关闭：两个 connector 工具消失，echo_alpha 不连坐。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": false}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"connector_find".to_string()),
        "总开关关时 connector_find 必须消失，实际 {names:?}"
    );
    assert!(
        !names.contains(&"connector_invoke".to_string()),
        "总开关关时 connector_invoke 必须消失，实际 {names:?}"
    );
    assert!(
        names.contains(&"echo_alpha".to_string()),
        "原生工具不受总开关影响（连接器≠闸门），实际 {names:?}"
    );

    // 运行时翻转（fresh-read）：改盘即恢复，无需重启。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": true}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        names.contains(&"connector_find".to_string()),
        "总开关重新打开后必须恢复供给，实际 {names:?}"
    );
}

// ---------------------------------------------------------------------------
// ③ dispatch 闸（B4）：总开关关闭时两工具被诚实拒绝
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_refuses_connector_tools_when_master_off() {
    let mut agent_loop = gate_loop();
    agent_loop.register_tool("connector_find".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(EchoTool));

    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": false}}"#);
    agent_loop.set_config_path(cfg);

    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "connector_find".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("连接器总开关已关闭"),
        "总开关关闭时 connector_find 必须诚实拒绝，实际: {out}"
    );

    // 未注册的 connector_invoke 同样按名字拦（闸在名字上，防在途请求漏网）。
    let tc2 = ToolCallInfo {
        id: "call_2".to_string(),
        name: "connector_invoke".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&tc2, &ctx).await;
    assert!(
        out.contains("连接器总开关已关闭"),
        "总开关关闭时 connector_invoke 同样拒绝，实际: {out}"
    );

    // 原生工具不受影响（连接器≠闸门）。
    let native = ToolCallInfo {
        id: "call_3".to_string(),
        name: "echo_alpha".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&native, &ctx).await;
    assert_eq!(out, "echo_ok", "原生工具照常执行，实际: {out}");

    // 翻开总开关 → 立即恢复执行（fresh-read，无需重启）。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": true}}"#);
    agent_loop.set_config_path(cfg);
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert_eq!(
        out, "echo_ok",
        "总开关恢复后 connector_find 必须真正执行，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// ④ 独占闸（B2 dispatch 端）：出处感知
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exclusive_gate_blocks_native_and_passes_binding_channel() {
    let mut agent_loop = gate_loop();
    agent_loop.register_tool("mcp_stub__read_page".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("echo_alpha".to_string(), Box::new(EchoTool));

    // 注入独占供给器（生产由 agent_factory 装配现扫闭包；测试直注固定表）。
    let hidden = vec!["mcp_stub__read_page".to_string()];
    agent_loop.set_exclusive_hidden_provider(std::sync::Arc::new(move || hidden.clone()));

    // 直调（web 通道）→ 独占闸拦。
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "mcp_stub__read_page".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("exclusively managed"),
        "独占表内工具直调必须被拦，实际: {out}"
    );

    // 绑定回灌（connector-binding 通道）→ 放行，真正执行（治理正向路由）。
    let bctx = RequestContext::new("connector-binding", "chat1", "session1", "sess1");
    let out = agent_loop.handle_tool_call(&tc, &bctx).await;
    assert_eq!(
        out, "echo_ok",
        "绑定回灌必须放行（拒了它独占一开连接器自己先瘫），实际: {out}"
    );

    // 表外工具任何通道不拦。
    let other = ToolCallInfo {
        id: "call_2".to_string(),
        name: "echo_alpha".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&other, &ctx).await;
    assert_eq!(out, "echo_ok", "表外工具不连坐，实际: {out}");

    let out = agent_loop.handle_tool_call(&other, &bctx).await;
    assert_eq!(out, "echo_ok", "表外工具绑定通道也照常，实际: {out}");
}

// ---------------------------------------------------------------------------
// ⑤ 供给器注入与缺省
// ---------------------------------------------------------------------------

#[test]
fn exclusive_provider_default_empty_and_wiring() {
    let agent_loop = gate_loop();
    // 缺省（未注入——独立装配形态，如本测试 / 集群 worker）→ 空表。
    // 治理补全 P1 起项目/selfbuilt loop 也经 register_connector_governance
    // 注入（全覆盖），不再是无注入的例子。
    assert!(agent_loop.current_exclusive_hidden_tools().is_empty());

    agent_loop.set_exclusive_hidden_provider(std::sync::Arc::new(|| {
        vec!["mcp_x__a".to_string(), "mcp_x__b".to_string()]
    }));
    assert_eq!(
        agent_loop.current_exclusive_hidden_tools(),
        vec!["mcp_x__a".to_string(), "mcp_x__b".to_string()]
    );
}

// ---------------------------------------------------------------------------
// ⑥ B4×B2 交叉（层级闸）：总开关关 = 独占闸一并释放（回原生直通）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn master_off_releases_exclusive_takeover() {
    let mut agent_loop = gate_loop();
    agent_loop.register_tool("mcp_stub__read_page".to_string(), Box::new(EchoTool));

    // 注入独占供给器（恒表——释放语义完全由 master 短路承担，供给器
    // 本身不感知总开关）。
    let hidden = vec!["mcp_stub__read_page".to_string()];
    agent_loop.set_exclusive_hidden_provider(std::sync::Arc::new(move || hidden.clone()));

    let dir = tempfile::tempdir().unwrap();

    // 总开关开：原生工具被独占接管——供给面消失 + 直调被拦（B2 基线）。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": true}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"mcp_stub__read_page".to_string()),
        "总开关开时独占工具必须隐藏，实际 {names:?}"
    );
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "mcp_stub__read_page".to_string(),
        arguments: "{}".to_string(),
    };
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("exclusively managed"),
        "总开关开时独占直调必须被拦，实际: {out}"
    );

    // 总开关关：独占闸一并释放（计划 B4 语义）——原生工具回供给面 + 直调
    // 真正执行（connector 工具同时下线 = 用户在系统级显式放弃监管，能力
    // 不允许砖死在「两边都调不到」）。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": false}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        names.contains(&"mcp_stub__read_page".to_string()),
        "总开关关时被独占的原生工具必须恢复供给，实际 {names:?}"
    );
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert_eq!(
        out, "echo_ok",
        "总开关关时原生直调必须恢复执行，实际: {out}"
    );

    // 总开关重开：独占接管即刻恢复（层级闸可逆，fresh-read）。
    let cfg = write_config(&dir, r#"{"connectors": {"enabled": true}}"#);
    agent_loop.set_config_path(cfg);
    let names = def_names(&agent_loop.build_tool_defs());
    assert!(
        !names.contains(&"mcp_stub__read_page".to_string()),
        "总开关重开后独占接管必须恢复，实际 {names:?}"
    );
}

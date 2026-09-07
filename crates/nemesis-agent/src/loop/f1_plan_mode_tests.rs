// F1 (devtool-upgrade 阶段 4)：plan/build 双模式测试。
//
// 覆盖面（对应实施计划 F1 验收）：
// ① AgentMode 解析/序列化（parse 大小写宽容 / 未知 None / 默认 Build）；
// ② 供给闸——Build 全量；Plan 只读白名单 ∪ mcp_* ∪ spawn；
// ③ tier × mode 正交——Mini ∩ Plan 双过滤同时生效（交集语义）；
// ④ dispatch 闸——Plan 下写类调用被拦并回灌引导文案；Build 切回后放行；
// ⑤ plans/ 唯一写放行——相对/绝对路径命中；越界与 `..` 逃逸拒绝；
//    无 workspace_root 一律拦截（诚实从严）；
// ⑥ slash 切换——gate_inbound /plan /build 短路回执 + 模式翻转 +
//    ModeChanged 事件发布（含事件访问器）；
// ⑦ Runtime Policy 快照带 mode 行（软提醒与硬闸互补）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;

/// 恒定回复的迷你 provider（同 f8 形状）。
struct PlanMockProvider;

#[async_trait]
impl LlmProvider for PlanMockProvider {
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

/// 迷你测试工具（执行成功返回固定文本——放行/拦截凭返回值即可区分）。
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("echo_ok".to_string())
    }
}

fn f1_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["read_file".to_string(), "write_file".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn f1_loop() -> AgentLoop {
    AgentLoop::new(Box::new(PlanMockProvider), f1_config())
}

fn def_names(defs: &[crate::types::ToolDefinition]) -> Vec<String> {
    defs.iter().map(|d| d.function.name.clone()).collect()
}

fn inbound_msg(content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "f1user".to_string(),
        chat_id: "f1chat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

// ---------------------------------------------------------------------------
// ① AgentMode 解析 / 序列化
// ---------------------------------------------------------------------------

#[test]
fn agent_mode_parse_and_as_str() {
    use crate::types::AgentMode;
    // 默认 Build。
    assert_eq!(AgentMode::default(), AgentMode::Build);
    // 线上形态往返。
    assert_eq!(AgentMode::Build.as_str(), "build");
    assert_eq!(AgentMode::Plan.as_str(), "plan");
    // 大小写 + 空白宽容。
    assert_eq!(AgentMode::parse("plan"), Some(AgentMode::Plan));
    assert_eq!(AgentMode::parse("  BUILD "), Some(AgentMode::Build));
    // 未知值 None（调用方回灌错误，不静默落回默认）。
    assert_eq!(AgentMode::parse("auto"), None);
    assert_eq!(AgentMode::parse(""), None);
}

#[test]
fn builtin_slash_commands_include_plan_build() {
    // 防回归：rewrite_custom_command 按 BUILTIN 跳过内置名——若被自定义
    // 命令表遮蔽，gate 的 /plan /build 臂永远轮不到（rewrite 先行改写）。
    assert!(AgentLoop::BUILTIN_SLASH_COMMANDS.contains(&"plan"));
    assert!(AgentLoop::BUILTIN_SLASH_COMMANDS.contains(&"build"));
}

// ---------------------------------------------------------------------------
// ② 供给闸：Plan 只读白名单 ∪ mcp_* ∪ spawn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_mode_supply_whitelist() {
    let mut al = f1_loop();
    // 白名单只读工具 + 写类 + MCP 前缀 + spawn + 非白名单普通工具。
    for name in [
        "read_file",
        "grep",
        "exec",
        "write_file",
        "mcp_srv_tool",
        "spawn",
        "custom_extra",
    ] {
        al.register_tool(name.to_string(), Box::new(EchoTool));
    }

    // Build 默认：全量供给。
    assert_eq!(al.mode(), crate::types::AgentMode::Build);
    let names = def_names(&al.build_tool_defs());
    for expected in [
        "read_file",
        "grep",
        "exec",
        "write_file",
        "mcp_srv_tool",
        "spawn",
        "custom_extra",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "Build 全量供给须含 {expected}，实际 {names:?}"
        );
    }

    // Plan：白名单 + mcp_* + spawn 幸存，其余消失。
    al.set_mode_with_event(
        crate::types::AgentMode::Plan,
        "agent:main:session:s",
        "web:s",
    );
    let names = def_names(&al.build_tool_defs());
    assert!(
        names.contains(&"read_file".to_string()) && names.contains(&"grep".to_string()),
        "白名单只读工具必须幸存，实际 {names:?}"
    );
    assert!(
        names.contains(&"mcp_srv_tool".to_string()) && names.contains(&"spawn".to_string()),
        "mcp_ 前缀与 spawn 必须豁免，实际 {names:?}"
    );
    assert!(
        !names.contains(&"exec".to_string()) && !names.contains(&"write_file".to_string()),
        "写类工具必须从供给剔除，实际 {names:?}"
    );
    assert!(
        !names.contains(&"custom_extra".to_string()),
        "非白名单工具必须从供给剔除，实际 {names:?}"
    );
}

// ---------------------------------------------------------------------------
// ③ tier × mode 正交：Mini ∩ Plan 交集
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tier_and_mode_filters_stack() {
    let mut al = f1_loop();
    for name in ["read_file", "grep", "exec", "write_file", "lsp", "mcp_x"] {
        al.register_tool(name.to_string(), Box::new(EchoTool));
    }
    al.set_tier(nemesis_types::capability::ModelTier::Mini);
    al.set_mode_with_event(
        crate::types::AgentMode::Plan,
        "agent:main:session:s",
        "web:s",
    );

    let names = def_names(&al.build_tool_defs());
    // read_file/grep：mini 允许 ∩ plan 白名单 → 幸存。
    for expected in ["read_file", "grep"] {
        assert!(
            names.contains(&expected.to_string()),
            "交集工具 {expected} 必须幸存，实际 {names:?}"
        );
    }
    // exec/write_file：mini 允许但 plan 白名单外 → 被模式层剔除。
    assert!(
        !names.contains(&"exec".to_string()) && !names.contains(&"write_file".to_string()),
        "plan 供给层必须剔除写类，实际 {names:?}"
    );
    // lsp：plan 白名单内但 mini 不允许 → 被 tier 层剔除。
    assert!(
        !names.contains(&"lsp".to_string()),
        "mini tier 层必须剔除白名单外的 lsp，实际 {names:?}"
    );
    // mcp_x：plan 豁免但 mini 不允许 → 被 tier 层剔除（豁免不越过 tier）。
    assert!(
        !names.contains(&"mcp_x".to_string()),
        "mcp_ 豁免不得越过 tier 过滤，实际 {names:?}"
    );
}

// ---------------------------------------------------------------------------
// ④ dispatch 闸：Plan 下写类被拦，Build 切回放行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_gate_blocks_write_in_plan_and_releases_in_build() {
    let mut al = f1_loop();
    al.register_tool("write_file".to_string(), Box::new(EchoTool));
    let ctx = RequestContext::new("web", "f1chat", "f1user", "agent:main:session:f1");
    let tc = ToolCallInfo {
        id: "call_1".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path": "src/main.rs", "content": "x"}"#.to_string(),
    };

    // Build：正常执行。
    let out = al.handle_tool_call(&tc, &ctx).await;
    assert_eq!(out, "echo_ok", "Build 模式写类必须执行，实际: {out}");

    // Plan：拦截 + 引导文案（含切回指引）。
    al.set_mode_with_event(
        crate::types::AgentMode::Plan,
        "agent:main:session:f1",
        "web:f1",
    );
    let out = al.handle_tool_call(&tc, &ctx).await;
    assert!(
        out.contains("Plan mode") && out.contains("/build"),
        "Plan 模式必须拦写类并回灌引导，实际: {out}"
    );

    // 切回 Build：同一调用立即放行（模式是运行时态，无需重启）。
    al.set_mode_with_event(
        crate::types::AgentMode::Build,
        "agent:main:session:f1",
        "web:f1",
    );
    let out = al.handle_tool_call(&tc, &ctx).await;
    assert_eq!(out, "echo_ok", "切回 Build 后写类必须恢复，实际: {out}");
}

// ---------------------------------------------------------------------------
// ⑤ plans/ 唯一写放行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plans_dir_exception_scoping() {
    let dir = tempfile::tempdir().unwrap();
    let mut al = f1_loop();
    al.register_tool("write_file".to_string(), Box::new(EchoTool));
    al.register_tool("edit_file".to_string(), Box::new(EchoTool));
    al.register_tool("grep".to_string(), Box::new(EchoTool));
    al.set_mode_with_event(
        crate::types::AgentMode::Plan,
        "agent:main:session:f1",
        "web:f1",
    );
    al.set_workspace_root(dir.path().to_path_buf());

    // 相对路径命中 plans/ 前缀 → 放行（唯一例外）。
    let tc_plans = ToolCallInfo {
        id: "c1".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path": "plans/todo.md"}"#.to_string(),
    };
    let out = al
        .handle_tool_call(&tc_plans, &RequestContext::new("web", "c", "u", "s"))
        .await;
    assert_eq!(out, "echo_ok", "plans/ 内 write_file 必须放行，实际: {out}");

    // 绝对路径命中 → 放行。
    let abs = dir.path().join("plans").join("abs.md");
    let tc_abs = ToolCallInfo {
        id: "c2".to_string(),
        name: "write_file".to_string(),
        arguments: format!(r#"{{"path": {}}}"#, serde_json::to_string(&abs).unwrap()),
    };
    let out = al
        .handle_tool_call(&tc_abs, &RequestContext::new("web", "c", "u", "s"))
        .await;
    assert_eq!(out, "echo_ok", "绝对路径 plans/ 内必须放行，实际: {out}");

    // 工作区内但 plans/ 外 → 拒。
    let tc_outside = ToolCallInfo {
        id: "c3".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path": "src/main.rs"}"#.to_string(),
    };
    let out = al
        .handle_tool_call(&tc_outside, &RequestContext::new("web", "c", "u", "s"))
        .await;
    assert!(out.contains("Plan mode"), "plans/ 外必须拒，实际: {out}");

    // `..` 逃逸（plans/../secret.txt 解析到根下）→ 拒。
    let tc_escape = ToolCallInfo {
        id: "c4".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path": "plans/../secret.txt"}"#.to_string(),
    };
    let out = al
        .handle_tool_call(&tc_escape, &RequestContext::new("web", "c", "u", "s"))
        .await;
    assert!(out.contains("Plan mode"), "plans/ 逃逸必须拒，实际: {out}");

    // 例外只认 write_file：其余写类即使落在 plans/ 也拒。
    let tc_edit = ToolCallInfo {
        id: "c5".to_string(),
        name: "edit_file".to_string(),
        arguments: r#"{"path": "plans/todo.md"}"#.to_string(),
    };
    let out = al
        .handle_tool_call(&tc_edit, &RequestContext::new("web", "c", "u", "s"))
        .await;
    assert!(
        out.contains("Plan mode"),
        "edit_file 无 plans/ 例外，实际: {out}"
    );

    // 只读工具不受模式闸影响（白名单供给 + dispatch 放行）。
    let tc_grep = ToolCallInfo {
        id: "c6".to_string(),
        name: "grep".to_string(),
        arguments: r#"{"pattern": "x"}"#.to_string(),
    };
    let out = al
        .handle_tool_call(&tc_grep, &RequestContext::new("web", "c", "u", "s"))
        .await;
    assert_eq!(out, "echo_ok", "Plan 模式只读工具必须放行，实际: {out}");
}

#[test]
fn plan_mode_write_allowed_unit_cases() {
    let dir = tempfile::tempdir().unwrap();
    let al = f1_loop();

    // 无 workspace_root（standalone）→ 一律拦截（诚实从严）。
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"path": "plans/a.md"}"#));

    al.set_workspace_root(dir.path().to_path_buf());
    // 坏 JSON / 缺 path / 非字符串 → 拒。
    assert!(!al.plan_mode_write_allowed("write_file", "not json"));
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"content": "x"}"#));
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"path": 42}"#));
    // 例外只认 write_file。
    assert!(!al.plan_mode_write_allowed("grep", r#"{"path": "plans/a.md"}"#));
    // 命中。
    assert!(al.plan_mode_write_allowed("write_file", r#"{"path": "plans/a.md"}"#));
}

// ---------------------------------------------------------------------------
// ⑥ slash 切换 + ModeChanged 事件
// ---------------------------------------------------------------------------

#[tokio::test]
async fn slash_plan_build_flip_and_publish_event() {
    let al = f1_loop();
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    al.set_agent_event_tx(Some(tx));

    // /plan：Immediate 回执 + 模式翻转 + 事件。
    match al.gate_inbound(&inbound_msg("/plan")) {
        GateOutcome::Immediate { agent_id, response } => {
            assert!(
                !agent_id.is_empty(),
                "agent_id 由 route_message 现取，不该为空: {agent_id:?}"
            );
            assert!(
                response.contains("Plan") && response.contains("/build"),
                "回执须含模式名与切回指引，实际: {response}"
            );
        }
        other => panic!("/plan 必须短路 Immediate，实际 {other:?}"),
    }
    assert_eq!(al.mode(), crate::types::AgentMode::Plan);
    let ev = rx.try_recv().expect("ModeChanged 必须发布");
    match &ev {
        nemesis_types::agent::AgentEvent::ModeChanged {
            session_key,
            chat_id,
            mode,
        } => {
            assert_eq!(chat_id, "f1chat");
            assert_eq!(mode, "plan");
            assert!(!session_key.is_empty(), "session_key 由 route_message 现取");
        }
        other => panic!("必须是 ModeChanged，实际 {other:?}"),
    }
    assert_eq!(ev.kind(), "ModeChanged");
    assert_eq!(ev.chat_id(), "f1chat");

    // /build：切回 + 第二个事件。
    match al.gate_inbound(&inbound_msg("/build")) {
        GateOutcome::Immediate { response, .. } => {
            assert!(
                response.contains("Build"),
                "回执须确认 Build，实际: {response}"
            );
        }
        other => panic!("/build 必须短路 Immediate，实际 {other:?}"),
    }
    assert_eq!(al.mode(), crate::types::AgentMode::Build);
    let ev = rx.try_recv().expect("第二个 ModeChanged");
    assert_eq!(ev.chat_id(), "f1chat");

    // 大小写敏感：/Plan 不匹配切换臂（精确匹配），也不匹配任何内置命令
    // （fallback None）→ 落正常消息准入，模式不翻转、事件不发布。
    assert!(matches!(
        al.gate_inbound(&inbound_msg("/Plan")),
        GateOutcome::Admitted(_)
    ));
    assert_eq!(al.mode(), crate::types::AgentMode::Build);
    assert!(rx.try_recv().is_err(), "/Plan 不得发布事件");
}

// ---------------------------------------------------------------------------
// ⑦ Runtime Policy 快照带 mode 行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_policy_snapshot_carries_mode_line() {
    let al = f1_loop();
    let instance = AgentInstance::new(f1_config());
    instance.set_history(vec![
        crate::types::ConversationTurn {
            role: "system".to_string(),
            content: "sys".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "t".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "t".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
    ]);

    // Build 默认：mode: build。
    let (msgs, _) = al.build_messages_with_memory_annotated(&instance, None);
    let joined = msgs.iter().map(|m| m.content.as_str()).collect::<String>();
    assert!(
        joined.contains("mode: build"),
        "策略快照须带 mode: build，实际: {joined}"
    );

    // Plan：mode 行换成 plan + 英文硬提醒（模型当轮可读）。
    al.set_mode_with_event(crate::types::AgentMode::Plan, "s", "c");
    let (msgs, _) = al.build_messages_with_memory_annotated(&instance, None);
    let joined = msgs.iter().map(|m| m.content.as_str()).collect::<String>();
    assert!(
        joined.contains("mode: plan（PLAN mode: do not modify files"),
        "策略快照须带 plan 软提醒，实际: {joined}"
    );
}

// ---------------------------------------------------------------------------
// C7（devtool-upgrade 阶段 6）：lsp 的 rename op 是 plan 模式写类
// ---------------------------------------------------------------------------

/// lsp 不在 PLAN_MODE_WRITE_TOOLS（整表拦会误伤只读 op）——rename 靠
/// args 探测特判。畸形 args 从严当写拦：plan 模式误拒成本（模型重试）
/// 远小于误放行（文件被改）。
#[test]
fn c7_is_lsp_write_call_detects_rename_op() {
    // rename → 写类。
    assert!(AgentLoop::is_lsp_write_call(
        "lsp",
        r#"{"op":"rename","path":"/x/a.rs","line":1,"character":2,"new_name":"y"}"#
    ));
    // 其余 lsp op（含 code_action 列表）只读，不拦。
    for op in [
        "definition",
        "references",
        "implementation",
        "hover",
        "code_action",
    ] {
        let args = format!(r#"{{"op":"{op}","path":"/x/a.rs","line":0,"character":0}}"#);
        assert!(
            !AgentLoop::is_lsp_write_call("lsp", &args),
            "{op} 是只读 op，plan 模式必须放行"
        );
    }
    // 非 lsp 工具与本判定无关（写类拦截走 PLAN_MODE_WRITE_TOOLS 表）。
    assert!(!AgentLoop::is_lsp_write_call(
        "write_file",
        r#"{"op":"rename"}"#
    ));
    // args 畸形 / 缺 op → 从严当写拦。
    assert!(AgentLoop::is_lsp_write_call("lsp", "{not json"));
    assert!(AgentLoop::is_lsp_write_call("lsp", r#"{"path":"/x/a.rs"}"#));
}

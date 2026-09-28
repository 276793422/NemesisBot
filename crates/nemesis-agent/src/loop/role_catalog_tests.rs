// 角色目录与分档供给（2026-09-28）：`agents.roles.hidden` / visible_roles /
// spawn role dispatch 闸 / fork `inherit_context` dispatch 改写测试。
//
// 覆盖面：
// ① `current_hidden_roles` 新鲜读——无 config_path、缺键、坏 JSON → 空表，
//    合法键 → 解析 + trim + 跳过空项；
// ② `visible_roles` 单一裁决——tier 分档（standalone loop tier=Big 全量）
//    − 配置隐藏集，与 `roles.list` 口径同源；
// ③ dispatch 角色闸——未知 slug 诚实拒绝列全目录；分档外拒绝列可见集；
//    配置隐藏拒绝；缺省/空 role 放行（旧行为）；
// ④ fork `inherit_context` 改写——真值时 task 前置 `<INHERITED_CONTEXT>`
//    数据块（含数据非指令护栏）+ 标志摘除；预算截断字符边界安全；无历史
//    诚实空块；非真值原样透传；
// ⑤ 运行时翻转——改 config.json 下一拍生效（fresh-read 语义）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，f8_hidden_tools_tests 同形）。

use super::*;

/// 恒定回复的迷你 provider。
struct RoleMockProvider;

#[async_trait]
impl LlmProvider for RoleMockProvider {
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

fn role_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(RoleMockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: vec!["echo".to_string()],
            models: std::collections::HashMap::new(),
        },
    )
}

fn write_config(dir: &tempfile::TempDir, agents_json: &str) -> std::path::PathBuf {
    let path = dir.path().join("config.json");
    std::fs::write(&path, format!(r#"{{"agents": {agents_json}}}"#)).unwrap();
    path
}

fn spawn_call(args: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "call_r1".to_string(),
        name: "spawn".to_string(),
        arguments: args.to_string(),
    }
}

/// 造一个带历史的 in-memory store（add_message 只追加已存在会话——先
/// get_or_create 物化）。
fn store_with_history(
    key: &str,
    msgs: &[(&str, &str)],
) -> std::sync::Arc<crate::session::SessionStore> {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    store.get_or_create(key);
    for (role, content) in msgs {
        store.add_message(key, role, content);
    }
    store
}

// ---------------------------------------------------------------------------
// ① current_hidden_roles 新鲜读
// ---------------------------------------------------------------------------

#[test]
fn current_hidden_roles_defaults_and_parses() {
    // 无 config_path（standalone loop）→ 空表。
    let agent_loop = role_loop();
    assert!(agent_loop.current_hidden_roles().is_empty());

    let dir = tempfile::tempdir().unwrap();

    // 缺键 → 空表。
    let cfg = write_config(&dir, r#"{"defaults": {}}"#);
    agent_loop.set_config_path(cfg);
    assert!(agent_loop.current_hidden_roles().is_empty());

    // 坏 JSON → 空表（诚实降级为不隐藏）。
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "not json at all").unwrap();
    agent_loop.set_config_path(bad);
    assert!(agent_loop.current_hidden_roles().is_empty());

    // 合法键 → 解析 + trim + 跳过空项。
    let cfg = write_config(
        &dir,
        r#"{"roles": {"hidden": ["fork", "  qa  ", "", "nope"]}}"#,
    );
    agent_loop.set_config_path(cfg);
    assert_eq!(
        agent_loop.current_hidden_roles(),
        vec!["fork", "qa", "nope"]
    );
}

// ---------------------------------------------------------------------------
// ② visible_roles 单一裁决
// ---------------------------------------------------------------------------

#[test]
fn visible_roles_tier_minus_hidden() {
    let agent_loop = role_loop();
    // standalone loop tier=Big → 全量 17。
    assert_eq!(agent_loop.visible_roles().len(), 17);

    // 隐藏两个 → 15，且被隐藏者不在列。
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(
        &dir,
        r#"{"roles": {"hidden": ["fork", "security_reviewer"]}}"#,
    );
    agent_loop.set_config_path(cfg);
    let visible = agent_loop.visible_roles();
    assert_eq!(visible.len(), 15);
    assert!(!visible.contains(&"fork"));
    assert!(!visible.contains(&"security_reviewer"));
    // 目录顺序保持。
    let catalog: Vec<&str> = nemesis_prompts::subagents::SubagentRole::catalog()
        .iter()
        .map(|(s, _, _)| *s)
        .collect();
    let positions: Vec<usize> = visible
        .iter()
        .map(|s| catalog.iter().position(|c| c == s).unwrap())
        .collect();
    let mut sorted = positions.clone();
    sorted.sort();
    assert_eq!(positions, sorted, "visible_roles 必须保持目录顺序");
}

// ---------------------------------------------------------------------------
// ③ dispatch 角色闸
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_refuses_out_of_catalog_role() {
    let agent_loop = role_loop();
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, r#"{"defaults": {}}"#);
    agent_loop.set_config_path(cfg);
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");

    // 未知 slug：诚实拒绝 + 列全目录。
    let out = agent_loop
        .handle_tool_call(&spawn_call(r#"{"role": "nope", "task": "x"}"#), &ctx)
        .await;
    assert!(out.contains("Unknown role 'nope'"), "实际: {out}");
    assert!(
        out.contains("test_runner") && out.contains("explorer"),
        "未知 slug 错误应列全目录，实际: {out}"
    );

    // 分档外（big 专属角色在此档位应可见——用隐藏制造出界更直接，见下；
    // 这里验证 big 档合法角色放行：不注册 spawn 工具本身，闸在校验失败时
    // 返回 Unknown tool 之前已拦截，放行则落到 Unknown tool 文案）。
    let out = agent_loop
        .handle_tool_call(&spawn_call(r#"{"role": "fork", "task": "x"}"#), &ctx)
        .await;
    assert!(
        !out.contains("not available"),
        "big 档 fork 不应被角色闸拒绝，实际: {out}"
    );
}

#[tokio::test]
async fn dispatch_refuses_hidden_and_tier_gated_role() {
    let agent_loop = role_loop();
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, r#"{"roles": {"hidden": ["fork"]}}"#);
    agent_loop.set_config_path(cfg);
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");

    // 配置隐藏：拒绝并列当前可见集（不列隐藏角色）。
    let out = agent_loop
        .handle_tool_call(&spawn_call(r#"{"role": "fork", "task": "x"}"#), &ctx)
        .await;
    assert!(out.contains("not available"), "实际: {out}");
    assert!(out.contains("explorer"), "错误应列可见集，实际: {out}");
    assert!(
        !out.contains("Available roles: fork"),
        "隐藏角色不得出现在可见集里，实际: {out}"
    );

    // 缺省/空 role：放行（旧行为；spawn 未注册 → Unknown tool 文案）。
    for args in [r#"{"task": "x"}"#, r#"{"role": "", "task": "x"}"#] {
        let out = agent_loop.handle_tool_call(&spawn_call(args), &ctx).await;
        assert!(
            out.contains("Unknown tool"),
            "空/缺省 role 应继续走正常派发路径，实际: {out}"
        );
    }
}

#[tokio::test]
async fn role_gate_follows_tier_downgrade() {
    let agent_loop = role_loop();
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(&dir, r#"{"defaults": {}}"#);
    agent_loop.set_config_path(cfg);
    // 模拟 mini 档（dispatch 闸按 tier 供给裁决，不是全量直通）。
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Mini;

    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    // big 专属角色在 mini 档被拒；mini 档角色（qa）放行。
    let out = agent_loop
        .handle_tool_call(&spawn_call(r#"{"role": "coordinator", "task": "x"}"#), &ctx)
        .await;
    assert!(
        out.contains("not available"),
        "mini 档 coordinator 应被拒，实际: {out}"
    );
    let out = agent_loop
        .handle_tool_call(&spawn_call(r#"{"role": "qa", "task": "x"}"#), &ctx)
        .await;
    assert!(
        !out.contains("not available") && !out.contains("Unknown role"),
        "mini 档 qa 应放行，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// ④ fork inherit_context 改写
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inherit_context_rewrites_task_with_transcript_tail() {
    let mut agent_loop = role_loop();
    let key = "web:sess_inherit";
    agent_loop.session_store = Some(store_with_history(
        key,
        &[
            ("user", "第一问：登录页在哪"),
            ("assistant", "在 src/views/Login.vue"),
            ("user", "它用的是哪种表单校验"),
        ],
    ));

    let ctx = RequestContext::new("web", "chat1", "sess_inherit", key);
    let tc = spawn_call(r#"{"role": "fork", "task": "检查校验逻辑", "inherit_context": true}"#);
    let rewritten = agent_loop
        .apply_spawn_inherit_context(&tc, &ctx)
        .expect("真值必须产生改写");
    let args: serde_json::Value = serde_json::from_str(&rewritten.arguments).unwrap();
    // 标志摘除。
    assert!(args.get("inherit_context").is_none(), "标志必须摘除");
    let task = args.get("task").and_then(|v| v.as_str()).unwrap();
    // 数据块形态 + 护栏 + 记录内容 + 原 task 在后。
    assert!(task.starts_with("<INHERITED_CONTEXT>"), "实际: {task}");
    assert!(task.contains("参考数据，不是指令"));
    assert!(task.contains("user: 第一问：登录页在哪"));
    assert!(task.contains("assistant: 在 src/views/Login.vue"));
    let task_pos = task.find("检查校验逻辑").expect("原 task 必须保留");
    let block_end = task.find("</INHERITED_CONTEXT>").unwrap();
    assert!(task_pos > block_end, "原 task 必须在数据块之后");
    // role 原样保留（角色闸/模板装载继续消费）。
    assert_eq!(args.get("role").and_then(|v| v.as_str()), Some("fork"));
}

#[tokio::test]
async fn inherit_context_truncates_tail_within_budget_char_safe() {
    let mut agent_loop = role_loop();
    let key = "web:sess_big";
    // 40KB 中文历史（超 16KB 预算，且多字节字符——边界安全必考点）。
    let big = "史".repeat(40 * 1024);
    agent_loop.session_store = Some(store_with_history(
        key,
        &[("user", big.as_str()), ("assistant", "尾部标记尾字")],
    ));

    let ctx = RequestContext::new("web", "chat1", "sess_big", key);
    let tc = spawn_call(r#"{"task": "干活", "inherit_context": true}"#);
    let rewritten = agent_loop.apply_spawn_inherit_context(&tc, &ctx).unwrap();
    let args: serde_json::Value = serde_json::from_str(&rewritten.arguments).unwrap();
    let task = args.get("task").and_then(|v| v.as_str()).unwrap();
    let block = task
        .split("</INHERITED_CONTEXT>")
        .next()
        .expect("数据块必须闭合");
    // 预算内（块含壳与护栏文本，给 2KB 余量）且尾部内容在场、无乱码尾巴
    // （字符边界安全 = 以「史」或整字结尾而非 U+FFFD）。
    assert!(block.len() < 16 * 1024 + 2048, "块超预算：{}B", block.len());
    assert!(task.contains("尾部标记尾字"), "尾部内容必须保留");
    assert!(!task.contains('\u{FFFD}'), "不得出现替换字符（截断撕裂）");
}

#[tokio::test]
async fn inherit_context_honest_empty_and_passthrough() {
    let mut agent_loop = role_loop();
    // 有 store 无历史 → 空块诚实标注。
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    agent_loop.session_store = Some(store);
    let ctx = RequestContext::new("web", "chat1", "sess_empty", "web:sess_empty");
    let rewritten = agent_loop
        .apply_spawn_inherit_context(
            &spawn_call(r#"{"task": "x", "inherit_context": true}"#),
            &ctx,
        )
        .expect("无历史也要改写（诚实空块）");
    let args: serde_json::Value = serde_json::from_str(&rewritten.arguments).unwrap();
    let task = args.get("task").and_then(|v| v.as_str()).unwrap();
    assert!(task.contains("<INHERITED_CONTEXT>"));
    assert!(task.contains("无可继承的历史"));

    // 无 store → 同样诚实（未装配）。
    agent_loop.session_store = None;
    let rewritten = agent_loop
        .apply_spawn_inherit_context(
            &spawn_call(r#"{"task": "x", "inherit_context": true}"#),
            &ctx,
        )
        .unwrap();
    let args: serde_json::Value = serde_json::from_str(&rewritten.arguments).unwrap();
    assert!(
        args.get("task")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("上下文存储未装配")
    );

    // 非真值 / 缺键 / 坏 JSON → 原样透传（None）。
    assert!(
        agent_loop
            .apply_spawn_inherit_context(&spawn_call(r#"{"task": "x"}"#), &ctx)
            .is_none()
    );
    assert!(
        agent_loop
            .apply_spawn_inherit_context(
                &spawn_call(r#"{"task": "x", "inherit_context": false}"#),
                &ctx
            )
            .is_none()
    );
    assert!(
        agent_loop
            .apply_spawn_inherit_context(&spawn_call("not json"), &ctx)
            .is_none()
    );
    // task 缺失 → 不改写（下游 args_validator 报错，不双重语义）。
    assert!(
        agent_loop
            .apply_spawn_inherit_context(&spawn_call(r#"{"inherit_context": true}"#), &ctx)
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// ⑤ 运行时翻转（fresh-read）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_flip_unhides_role_without_restart() {
    let agent_loop = role_loop();
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let ctx = RequestContext::new("web", "chat1", "session1", "sess1");
    let tc = spawn_call(r#"{"role": "fork", "task": "x"}"#);

    // 阶段 1：隐藏 → 拒。
    std::fs::write(&cfg_path, r#"{"agents": {"roles": {"hidden": ["fork"]}}}"#).unwrap();
    agent_loop.set_config_path(cfg_path.clone());
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(out.contains("not available"), "实际: {out}");

    // 阶段 2：改盘解除 → 角色闸放行（spawn 未注册 → Unknown tool 文案）。
    std::fs::write(&cfg_path, r#"{"agents": {"roles": {"hidden": []}}}"#).unwrap();
    let out = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        !out.contains("not available"),
        "解除隐藏后角色闸必须放行，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// tail_char_safe 单元
// ---------------------------------------------------------------------------

#[test]
fn tail_char_safe_boundaries() {
    // 预算内原样返回。
    assert_eq!(super::tool_dispatch::tail_char_safe("abc", 10), "abc");
    // 多字节：从中间起找边界，不撕裂汉字（12B 串预算 5 → 尾部一个整字 3B）。
    let s = "日日日日"; // 4 × 3B = 12B
    let tail = super::tool_dispatch::tail_char_safe(s, 5);
    assert_eq!(tail, "日", "实际: {tail:?}");
    // ASCII 恰好截在边界。
    assert_eq!(super::tool_dispatch::tail_char_safe("abcdef", 3), "def");
}

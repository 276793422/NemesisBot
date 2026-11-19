//! hooks 工具单元测试：解析（合法/非法/event 枚举/timeout 类型/prompt 型
//! 校验）、CRUD 落盘形态（统一存储——工具专属圈定组 scope.sessions == [sid]，
//! id 跨事件递增）、schema 无 session 参数（只能给自己设的结构保证）、
//! operation_override 投影（list=internal）、min_tier、即刻生效文案（2026-11-20 桥恒在）、
//! tier 白名单成员、孤儿对账函数。
//!
//! 测试访问边界（外部 crate）：只用 cc_hooks pub API（list_session_hooks /
//! add_session_hook / parse_cc_hooks / global_hook_counts）+ hooks.json 原文
//! 断言——CcEvents/组结构私有字段不可触。

use super::*;
use nemesis_agent::r#loop::Tool as _;

fn tempdir() -> std::path::PathBuf {
    tempfile::tempdir().expect("tempdir").keep()
}

fn tool(config_dir: &std::path::Path) -> HooksTool {
    HooksTool::new(config_dir.to_path_buf())
}

/// 主会话形态的 ctx（web 键 `agent:main:session:<sid>`）。
fn ctx_main(sid: &str) -> RequestContext {
    nemesis_agent::context::RequestContext::new(
        "web",
        "chat-1",
        "tester",
        &format!("{WEB_SESSION_KEY_PREFIX}{sid}"),
    )
}

async fn add(tool: &HooksTool, ctx: &RequestContext, event: &str, command: &str) -> String {
    tool.execute(
        &serde_json::json!({ "action": "add", "event": event, "command": command }).to_string(),
        ctx,
    )
    .await
    .expect("add ok")
}

/// hooks.json 原文（统一存储落盘断言用）。
fn raw_hooks(cfg: &std::path::Path) -> String {
    std::fs::read_to_string(cfg.join(nemesis_agent::cc_hooks::HOOKS_FILE)).expect("hooks.json")
}

// ---------------------------------------------------------------------------
// parse_hooks_args
// ---------------------------------------------------------------------------

#[test]
fn parse_add_minimal_defaults() {
    let cmd = parse_hooks_args(r#"{"action":"add","event":"PreToolUse","command":"echo hi"}"#)
        .expect("parse");
    match cmd {
        HooksCommand::Add {
            event,
            command,
            prompt,
            matcher,
            timeout,
        } => {
            assert_eq!(event, "PreToolUse");
            assert_eq!(command, "echo hi");
            assert_eq!(prompt, "");
            assert_eq!(matcher, None);
            assert_eq!(timeout, None);
        }
        _ => panic!("expected Add"),
    }
}

#[test]
fn parse_add_full_and_trims() {
    let cmd = parse_hooks_args(
        r#"{"action":"add","event":"  Stop ","command":" echo hi ","matcher":"  ","timeout":120}"#,
    )
    .expect("parse");
    match cmd {
        HooksCommand::Add {
            event,
            command,
            prompt,
            matcher,
            timeout,
        } => {
            assert_eq!(event, "Stop");
            assert_eq!(command, "echo hi");
            assert_eq!(prompt, "");
            // 纯空白 matcher = None（= 全部工具）。
            assert_eq!(matcher, None);
            assert_eq!(timeout, Some(120));
        }
        _ => panic!("expected Add"),
    }
}

#[test]
fn parse_rejects_unknown_action_and_missing_fields() {
    assert!(
        parse_hooks_args(r#"{"action":"nope"}"#)
            .err()
            .unwrap()
            .contains("未知 action")
    );
    assert!(
        parse_hooks_args("{}")
            .err()
            .unwrap()
            .contains("missing field: action")
    );
    assert!(
        parse_hooks_args(r#"{"action":"add","event":"PreToolUse"}"#)
            .err()
            .unwrap()
            .contains("command")
    );
    assert!(
        parse_hooks_args(r#"{"action":"add","command":"x"}"#)
            .err()
            .unwrap()
            .contains("event")
    );
    assert!(
        parse_hooks_args(r#"{"action":"delete"}"#)
            .err()
            .unwrap()
            .contains("id")
    );
    assert!(
        parse_hooks_args("not json")
            .err()
            .unwrap()
            .contains("invalid JSON")
    );
}

#[test]
fn parse_rejects_unknown_event_and_bad_timeout() {
    let err = parse_hooks_args(r#"{"action":"add","event":"NoSuchEvent","command":"x"}"#)
        .err()
        .unwrap();
    assert!(err.contains("未知 hook 事件名"));
    // 12 事件全名单都在错误提示里可回查。
    for ev in SESSION_HOOK_EVENTS {
        assert!(err.contains(ev), "错误提示应含合法事件 {ev}");
    }
    let err = parse_hooks_args(r#"{"action":"add","event":"Stop","command":"x","timeout":"soon"}"#)
        .err()
        .unwrap();
    assert!(err.contains("正整数秒"));
}

// ---------------------------------------------------------------------------
// parse：prompt 型钩子（LLM 执行体）校验
// ---------------------------------------------------------------------------

#[test]
fn parse_add_prompt_hook_on_stop_ok() {
    let cmd = parse_hooks_args(
        r#"{"action":"add","event":"Stop","prompt":"继续检查代码是否正确，确认是否有问题"}"#,
    )
    .expect("prompt add parses");
    match cmd {
        HooksCommand::Add {
            event,
            command,
            prompt,
            matcher,
            timeout: _,
        } => {
            assert_eq!(event, "Stop");
            assert_eq!(command, "", "prompt 型条目 command 恒空");
            assert_eq!(prompt, "继续检查代码是否正确，确认是否有问题");
            assert_eq!(matcher, None);
        }
        _ => panic!("expected Add"),
    }
}

#[test]
fn parse_add_rejects_both_empty() {
    // command 与 prompt 双空 = 没有执行体，拒绝。
    let err = parse_hooks_args(r#"{"action":"add","event":"Stop"}"#)
        .err()
        .unwrap();
    assert!(err.contains("至少一项非空"), "err={err}");
    // command 空串 + prompt 缺席同罪。
    let err = parse_hooks_args(r#"{"action":"add","event":"Stop","command":"  "}"#)
        .err()
        .unwrap();
    assert!(err.contains("至少一项非空"));
}

#[test]
fn parse_add_rejects_both_provided() {
    // Exactly one 契约：双非空混合条目语义未定（shell 执行路径不读 prompt
    // → command/prompt 必有一侧被静默忽略）——loud 拒绝，不静默偏向任何一型。
    let err = parse_hooks_args(
        r#"{"action":"add","event":"Stop","command":"echo hi","prompt":"继续检查"}"#,
    )
    .err()
    .unwrap();
    assert!(err.contains("二选一"), "err={err}");
    assert!(err.contains("不可混合"), "err={err}");
}

#[test]
fn parse_add_rejects_prompt_off_stop() {
    // prompt 型仅 Stop 合法——只有 Stop 有「turn 结束」时机。
    for event in [
        "PreToolUse",
        "SessionStart",
        "UserPromptSubmit",
        "SessionEnd",
    ] {
        let err = parse_hooks_args(&format!(
            r#"{{"action":"add","event":"{event}","prompt":"x"}}"#
        ))
        .err()
        .unwrap();
        assert!(err.contains("Stop"), "event={event} err={err}");
    }
    // command 型不受此限（任意事件合法）。
    assert!(
        parse_hooks_args(r#"{"action":"add","event":"PreToolUse","command":"echo ok"}"#).is_ok()
    );
}

// ---------------------------------------------------------------------------
// execute：add/delete/list 落盘形态（统一存储）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_writes_store_with_incrementing_ids() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-abc");
    let out1 = add(&t, &ctx, "PreToolUse", "echo one").await;
    assert!(out1.contains("h1"), "首个 id = h1\n{out1}");
    let out2 = add(&t, &ctx, "Stop", "echo two").await;
    assert!(out2.contains("h2"), "跨事件递增 = h2\n{out2}");

    // 查询面：本会话 2 条（跨事件组），数组序 = SESSION_HOOK_EVENTS 定序。
    let views = list_session_hooks(&cfg, "s-abc").expect("list");
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].id, "h1");
    assert_eq!(views[0].event, "PreToolUse");
    assert_eq!(views[0].command, "echo one");
    assert_eq!(views[1].id, "h2");
    assert_eq!(views[1].event, "Stop");
    assert_eq!(
        views[1].timeout,
        Some(DEFAULT_TIMEOUT_SECS),
        "缺省超时写 60s"
    );

    // 落盘形态（统一存储）：圈定是结构性事实——scope.sessions 落在
    // hooks.json 里（scope-is-storage），条目自身不携带会话键。
    let raw = raw_hooks(&cfg);
    assert!(
        raw.contains("scope"),
        "统一存储形态：圈定落 scope 键\n{raw}"
    );
    assert!(raw.contains("s-abc"), "圈定组含会话 sid");

    // 工具专属组不进全局分账（global_hook_counts 只数未圈定组）。
    let events = nemesis_agent::cc_hooks::parse_cc_hooks(&raw).expect("parse");
    assert!(
        events.global_hook_counts().is_empty(),
        "工具圈定组不得计入全局钩子"
    );
}

#[tokio::test]
async fn add_uses_full_key_when_no_prefix() {
    let cfg = tempdir();
    let t = tool(&cfg);
    // 无 web 前缀的键（RPC/通道形态）→ 整键剥前缀兜底 = 裸键本身作圈定 sid。
    let ctx = nemesis_agent::context::RequestContext::new("rpc", "c", "u", "plain-sess");
    let out1 = add(&t, &ctx, "Stop", "echo x").await;
    assert!(out1.contains("h1"), "\n{out1}");

    // web 整键形态剥前缀后落同一 sid——同一会话双口径一致（h2 连续递增）。
    let out2 = add(&t, &ctx_main("plain-sess"), "PreToolUse", "echo y").await;
    assert!(out2.contains("h2"), "剥前缀同 sid → id 连续递增\n{out2}");
    assert_eq!(
        list_session_hooks(&cfg, "plain-sess").expect("list").len(),
        2
    );
}

#[tokio::test]
async fn add_timeout_zero_rejected_and_over_cap_clamped() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-t");
    let err = t
        .execute(
            r#"{"action":"add","event":"Stop","command":"x","timeout":0}"#,
            &ctx,
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("正秒数"));

    let out = t
        .execute(
            r#"{"action":"add","event":"Stop","command":"x","timeout":99999}"#,
            &ctx,
        )
        .await
        .expect("clamped add");
    assert!(out.contains("600"), "响应注明钳到上限\n{out}");
    let views = list_session_hooks(&cfg, "s-t").expect("list");
    assert_eq!(views.len(), 1);
    assert_eq!(
        views[0].timeout,
        Some(MAX_TIMEOUT_SECS),
        "落盘条目被钳到上限"
    );
}

// ---------------------------------------------------------------------------
// execute：prompt 型钩子（LLM 执行体）落盘 + 渲染
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_prompt_hook_writes_entry_and_renders_type() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-loop");
    let out = t
        .execute(
            r#"{"action":"add","event":"Stop","prompt":"继续检查代码是否正确，确认是否有问题"}"#,
            &ctx,
        )
        .await
        .expect("prompt add ok");
    assert!(out.contains("提示词型"), "渲染标注类型\n{out}");
    assert!(
        out.contains("没有轮数上限"),
        "响应说明永续语义与停止方式\n{out}"
    );

    // 落盘形态：command 空 + prompt 非空（同一 CcCommand 结构二型分派）。
    let views = list_session_hooks(&cfg, "s-loop").expect("list");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].command, "");
    assert_eq!(views[0].prompt, "继续检查代码是否正确，确认是否有问题");

    // list 渲染 prompt 型（预览 + 提示词型标注），不显示超时/shell。
    let listing = t
        .execute(r#"{"action":"list"}"#, &ctx)
        .await
        .expect("list ok");
    assert!(listing.contains("提示词型"), "list 标注类型\n{listing}");
    assert!(
        listing.contains("继续检查代码"),
        "list 含 prompt 预览\n{listing}"
    );

    // delete 走同一条按 id 路径（prompt 型与 command 型同构）。
    let del = t
        .execute(r#"{"action":"delete","id":"h1"}"#, &ctx)
        .await
        .expect("delete ok");
    assert!(del.contains("提示词型"), "delete 渲染条目类型\n{del}");
    assert!(
        !raw_hooks(&cfg).contains("s-loop"),
        "全空圈定组整组摘除（统一存储）"
    );
}

#[tokio::test]
async fn add_session_start_honest_next_session_note() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-ss");
    let out = add(&t, &ctx, "SessionStart", "echo boot").await;
    assert!(
        out.contains("下个会话"),
        "mid-session 添加 SessionStart 必须诚实注明下会话生效\n{out}"
    );
}

#[tokio::test]
async fn delete_removes_entry_and_cleans_empty_key() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-del");
    add(&t, &ctx, "PreToolUse", "echo a").await;
    add(&t, &ctx, "Stop", "echo b").await;

    let out = t
        .execute(r#"{"action":"delete","id":"h1"}"#, &ctx)
        .await
        .expect("delete ok");
    assert!(out.contains("h1") && out.contains("已删除"));

    // h1 摘除后 h2 仍在；再删 h2 → 工具专属圈定组整组摘除（统一存储）。
    assert_eq!(list_session_hooks(&cfg, "s-del").expect("list").len(), 1);
    t.execute(r#"{"action":"delete","id":"h2"}"#, &ctx)
        .await
        .expect("delete ok");
    assert!(list_session_hooks(&cfg, "s-del").expect("list").is_empty());
    let raw = raw_hooks(&cfg);
    assert!(!raw.contains("s-del"), "空壳圈定组整组摘除\n{raw}");
}

#[tokio::test]
async fn delete_missing_id_is_honest_with_existing_ids() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-miss");

    // 空会话 delete → 「没有已设置的钩子」。
    let err = t
        .execute(r#"{"action":"delete","id":"h1"}"#, &ctx)
        .await
        .err()
        .unwrap();
    assert!(err.contains("没有已设置的钩子"));

    add(&t, &ctx, "Stop", "echo b").await;
    let err = t
        .execute(r#"{"action":"delete","id":"h9"}"#, &ctx)
        .await
        .err()
        .unwrap();
    assert!(
        err.contains("h9") && err.contains("h1"),
        "报错列出现有 id\n{err}"
    );
}

#[tokio::test]
async fn list_reports_and_scopes_honestly() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = ctx_main("s-list");

    let out = t
        .execute(r#"{"action":"list"}"#, &ctx)
        .await
        .expect("list ok");
    assert!(out.contains("没有已设置的钩子") && out.contains("Dashboard Hooks 页"));

    add(&t, &ctx, "PreToolUse", "echo a").await;
    let out = t
        .execute(r#"{"action":"list"}"#, &ctx)
        .await
        .expect("list ok");
    assert!(out.contains("只对本会话生效"), "诚实注明圈定范围\n{out}");
    assert!(out.contains("echo a") && out.contains("h1"));
    assert!(out.contains("全部工具"), "空 matcher 渲染为全部工具");
}

#[tokio::test]
async fn empty_session_key_is_honest_error() {
    let cfg = tempdir();
    let t = tool(&cfg);
    let ctx = nemesis_agent::context::RequestContext::new("web", "c", "u", "");
    let err = t.execute(r#"{"action":"list"}"#, &ctx).await.err().unwrap();
    assert!(err.contains("session_key"), "无会话身份诚实拒绝\n{err}");
}

#[tokio::test]
async fn activation_note_immediate_effect_wording() {
    // 2026-11-20 即刻生效语义：桥恒在（load_from_dir 缺失/坏文件也建桥
    // 待命）——add 响应恒「落盘即热更」，无「重启 Agent」分支文案。
    let cfg = tempdir();
    let ctx = ctx_main("s-word");
    let t = tool(&cfg);
    let out = add(&t, &ctx, "Stop", "echo x").await;
    assert!(out.contains("下条消息起生效"), "即刻生效文案\n{out}");
    assert!(
        !out.contains("重启"),
        "无重启分支（旧冷启动文案退役）\n{out}"
    );
}

// ---------------------------------------------------------------------------
// 结构保证：schema 无 session 参数；安全投影；tier 白名单
// ---------------------------------------------------------------------------

#[test]
fn schema_has_no_session_parameter_by_design() {
    let t = tool(&tempdir());
    let schema = t.parameters();
    let props = schema["properties"].as_object().expect("properties");
    assert!(
        !props.contains_key("session") && !props.contains_key("session_id"),
        "schema 不得有 session 入参——只能给自己设是结构保证"
    );
    assert_eq!(props["action"]["enum"][0], "add");
    // event 枚举 = 12 事件全量（与 SESSION_HOOK_EVENTS 同序）。
    let ev_enum: Vec<&str> = props["event"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ev_enum.as_slice(), &SESSION_HOOK_EVENTS);
    assert_eq!(schema["required"], serde_json::json!(["action"]));
}

#[test]
fn operation_override_list_internal_add_delete_forced_card() {
    let t = tool(&tempdir());
    let ov = t
        .operation_override(r#"{"action":"list"}"#)
        .expect("list projects");
    assert_eq!(ov.op_type, "internal");
    assert_eq!(ov.force_approval, None);

    // add/delete：file_write + 强审批旗标（默认策略 file_write=allow 只
    // 审计——钩子安装/移除必须过人，旗标把卡变真）。
    for args in [
        r#"{"action":"add","event":"Stop","command":"x"}"#,
        r#"{"action":"delete","id":"h1"}"#,
    ] {
        let ov = t.operation_override(args).expect("add/delete project");
        assert_eq!(ov.op_type, "file_write", "args={args}");
        assert!(
            ov.force_approval.as_deref().is_some_and(|s| !s.is_empty()),
            "add/delete 必须带非空强审批理由（默认策略下出真卡），args={args}"
        );
    }
    // 解析失败 → None（落静态 FileWrite 声明，语义层报错）。
    assert!(t.operation_override("not json").is_none());
    assert!(t.operation_override(r#"{"action":"nope"}"#).is_none());
}

#[test]
fn min_tier_is_normal() {
    let t = tool(&tempdir());
    assert_eq!(t.min_tier(), Some("normal"));
    // Normal 档白名单含 hooks（capability 单一真相源）。
    assert!(
        nemesis_types::capability::tier_allowed_tools(nemesis_types::capability::ModelTier::Normal)
            .contains(&TOOL_NAME)
    );
}

// ---------------------------------------------------------------------------
// 孤儿对账（reconcile_session_orphans）——启动对账的行为钉（统一存储）
// ---------------------------------------------------------------------------

#[test]
fn reconcile_orphans_removes_dead_sessions_and_keeps_live() {
    let cfg = tempdir();
    // 统一存储：经生产 RMW API 种两个会话的工具专属圈定组。
    let id = nemesis_agent::cc_hooks::add_session_hook(
        &cfg,
        "live-1",
        "Stop",
        "",
        "echo live",
        "",
        Some(60),
    )
    .expect("seed live");
    assert_eq!(id, "h1");
    let id = nemesis_agent::cc_hooks::add_session_hook(
        &cfg,
        "dead-1",
        "Stop",
        "",
        "echo dead",
        "",
        Some(60),
    )
    .expect("seed dead");
    assert_eq!(id, "h1", "不同会话各自工具组 id 独立起算");

    let live: std::collections::HashSet<String> = ["live-1".to_string()].into();
    let removed = nemesis_agent::cc_hooks::reconcile_session_orphans(&cfg, &live);
    assert_eq!(removed, 1, "摘除死者圈定组");

    assert_eq!(list_session_hooks(&cfg, "live-1").expect("live").len(), 1);
    assert!(list_session_hooks(&cfg, "dead-1").expect("dead").is_empty());

    // 幂等：再跑一遍 0 摘除。
    assert_eq!(
        nemesis_agent::cc_hooks::reconcile_session_orphans(&cfg, &live),
        0
    );
}

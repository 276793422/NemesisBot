//! hooks 工具单元测试：解析（合法/非法/event 枚举/timeout 类型）、CRUD
//! 落盘形态（id 递增/裸 sid 键/删除清壳）、schema 无 session 参数（只能
//! 给自己设的结构保证）、operation_override 投影（list=internal）、
//! min_tier、桥在/不在的生效文案、tier 白名单成员、孤儿对账函数。

use super::*;
use nemesis_agent::cc_hooks::session_hooks_path;
use nemesis_agent::r#loop::Tool as _;
use std::collections::BTreeMap;

fn tempdir() -> std::path::PathBuf {
    tempfile::tempdir().expect("tempdir").keep()
}

fn tool(config_dir: &std::path::Path) -> HooksTool {
    HooksTool::new(config_dir.to_path_buf(), None)
}

fn tool_with_bridge(config_dir: &std::path::Path) -> HooksTool {
    let bridge =
        nemesis_agent::cc_hooks::CcHookBridge::from_json(r#"{"hooks":{}}"#, std::env::temp_dir())
            .expect("empty bridge");
    HooksTool::new(config_dir.to_path_buf(), Some(std::sync::Arc::new(bridge)))
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
            matcher,
            timeout,
        } => {
            assert_eq!(event, "PreToolUse");
            assert_eq!(command, "echo hi");
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
            matcher,
            timeout,
        } => {
            assert_eq!(event, "Stop");
            assert_eq!(command, "echo hi");
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
// execute：add/delete/list 落盘形态
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

    // 落盘形态：sid → event → entries（BTreeMap 键序确定）。
    let store = read_session_hook_store(&cfg);
    let per = store.hooks.get("s-abc").expect("sid key");
    assert_eq!(per.get("PreToolUse").unwrap()[0].command, "echo one");
    assert_eq!(
        per.get("Stop").unwrap()[0].timeout,
        Some(DEFAULT_TIMEOUT_SECS)
    );
    // scope 不落存储——圈定是按 sid 键的结构性事实，无需逐条 scope。
    assert!(
        !serde_json::to_string(&store).unwrap().contains("scope"),
        "存储形态不应携带 scope 键"
    );
}

#[tokio::test]
async fn add_uses_full_key_when_no_prefix() {
    let cfg = tempdir();
    let t = tool(&cfg);
    // 无 web 前缀的键（RPC/通道形态）→ 整键作存储键（桥侧同款双形态）。
    let ctx = nemesis_agent::context::RequestContext::new("rpc", "c", "u", "plain-sess");
    add(&t, &ctx, "Stop", "echo x").await;
    let store = read_session_hook_store(&cfg);
    assert!(store.hooks.contains_key("plain-sess"));
    assert!(!store.hooks.contains_key("agent:main:session:plain-sess"));
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
    let store = read_session_hook_store(&cfg);
    assert_eq!(
        store.hooks.get("s-t").unwrap().get("Stop").unwrap()[0].timeout,
        Some(MAX_TIMEOUT_SECS)
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

    // h1 摘除后 h2 仍在；再删 h2 → 空壳整键摘除。
    let store = read_session_hook_store(&cfg);
    assert_eq!(store.session_len("s-del"), 1);
    t.execute(r#"{"action":"delete","id":"h2"}"#, &ctx)
        .await
        .expect("delete ok");
    let store = read_session_hook_store(&cfg);
    assert!(!store.hooks.contains_key("s-del"), "空壳键应整键摘除");
    assert!(store.is_empty());
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
async fn bridge_presence_switches_activation_wording() {
    let cfg = tempdir();
    let ctx = ctx_main("s-word");
    let cold = tool(&cfg);
    let out = add(&cold, &ctx, "Stop", "echo x").await;
    assert!(
        out.contains("重启 Agent 后生效"),
        "冷启动无桥诚实降级\n{out}"
    );

    // 同目录换桥在形态：落盘即热更文案（bridge 不实际读盘——文案语义）。
    std::fs::remove_file(session_hooks_path(&cfg)).unwrap();
    let warm = tool_with_bridge(&cfg);
    let out = add(&warm, &ctx, "Stop", "echo y").await;
    assert!(out.contains("下条消息起生效"), "桥在 = 即时生效文案\n{out}");
    assert!(!out.contains("重启"));
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
// 孤儿对账（reconcile_session_orphans）——批次三启动对账的行为钉
// ---------------------------------------------------------------------------

#[test]
fn reconcile_orphans_removes_dead_sessions_and_keeps_live() {
    let cfg = tempdir();
    let mut store = SessionHookStore::default();
    store.hooks.insert(
        "live-1".into(),
        BTreeMap::from([(
            "Stop".into(),
            vec![SessionHookEntry {
                id: "h1".into(),
                matcher: String::new(),
                command: "echo live".into(),
                timeout: Some(60),
            }],
        )]),
    );
    store.hooks.insert(
        "dead-1".into(),
        BTreeMap::from([(
            "Stop".into(),
            vec![SessionHookEntry {
                id: "h1".into(),
                matcher: String::new(),
                command: "echo dead".into(),
                timeout: Some(60),
            }],
        )]),
    );
    write_session_hook_store(&cfg, &store).expect("seed write");

    let live: std::collections::HashSet<String> = ["live-1".to_string()].into();
    let removed = nemesis_agent::cc_hooks::reconcile_session_orphans(&cfg, &live);
    assert_eq!(removed, 1);

    let after = read_session_hook_store(&cfg);
    assert!(after.hooks.contains_key("live-1"));
    assert!(!after.hooks.contains_key("dead-1"));

    // 幂等：再跑一遍 0 摘除。
    assert_eq!(
        nemesis_agent::cc_hooks::reconcile_session_orphans(&cfg, &live),
        0
    );
}

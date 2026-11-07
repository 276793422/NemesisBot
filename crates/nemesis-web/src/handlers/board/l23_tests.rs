//! （三期 2026-11-03）：autopilot 父会话绑定 handler 层测试。
//!
//! home 单例经 HomeGuard 重定向到 tempdir（sessions_snooze_tests 同款纪律，
//! test_home::lock_home 可重入锁跨测试串行）：chat_log 的 append/read/clear
//! 全走 default_path_manager 单例，fire 侧存活判定与注入读同一寻址路径。
//! fire 走 fire_autopilot 单一真相源（cluster 版直调 None cluster——target
//! 空 + estop 未装配，离线可跑；非 cluster 编译只跑不依赖 fire 的子集）。

use super::*;
use crate::handlers::sessions::SessionsHandler;
use crate::ws_router::ModuleHandler;
use nemesis_board::models::priority;
use std::path::{Path, PathBuf};

struct HomeGuard {
    old: PathBuf,
    _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.old.clone());
    }
}

/// 重定向单例 home 到 tempdir（workspace = `<home>/workspace` 与单例派生
/// 规则一致——chat_log 落 `<home>/workspace/logs/session_logs/`）。
fn isolate(home: &Path) -> HomeGuard {
    let lock = crate::test_home::lock_home();
    let old = nemesis_path::default_path_manager().home_dir();
    nemesis_path::default_path_manager().set_home_dir(home.to_path_buf());
    HomeGuard { old, _lock: lock }
}

fn store_of(ctx: &RequestContext) -> Arc<BoardStore> {
    ctx.state.board.as_ref().unwrap().store().clone()
}

async fn ws(
    ctx: &RequestContext,
    module: &str,
    cmd: &str,
    data: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    match module {
        "board" => super::tests::dispatch(ctx, cmd, data).await,
        "sessions" => SessionsHandler.handle_cmd(cmd, Some(data), ctx).await,
        _ => unreachable!("l23_tests 只驱动 board/sessions 两模块"),
    }
}

fn new_ap(name: &str, session_key: Option<String>, enabled: bool) -> nemesis_board::NewAutopilot {
    nemesis_board::NewAutopilot {
        name: name.to_string(),
        cron: "0 9 * * *".to_string(),
        title: "巡检 {date}".to_string(),
        description: "原始描述".to_string(),
        priority: priority::MEDIUM,
        project_id: None,
        target: String::new(),
        enabled,
        auto_plan: false,
        acceptance_criteria: None,
        session_key,
        mode: nemesis_board::models::AUTOPILOT_MODE_LIVE.to_string(),
    }
}

/// 绑定 + 父会话存活 → fire 建单 description 注入父会话溯源行 + 尾窗摘录
/// （既有描述在前、块在后）；规则保持启用；二次触发稳定。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn l23_bound_rule_injects_parent_context_into_issue() {
    let dir = super::tests::unique_dir("l23-inject");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let key = "agent:main:session:l23src";
    nemesis_agent::chat_log::append_chat_log(key, "user", "请检查构建日志");
    nemesis_agent::chat_log::append_chat_log(key, "assistant", "已检查，发现了 3 个警告");

    let ap = store
        .create_autopilot(&new_ap("绑定规则", Some(key.to_string()), true))
        .unwrap();
    let actor = nemesis_board::assignment::Actor::admin("admin");
    let out = fire_autopilot(&store, None, &ap, &actor, None).unwrap();
    assert_eq!(out["ran"], true);
    let issue = store.get_issue(out["issue_id"].as_i64().unwrap()).unwrap();
    assert!(
        issue
            .description
            .contains("父会话上下文（autopilot 绑定继承）"),
        "{}",
        issue.description
    );
    assert!(
        issue.description.contains(&format!("父会话: {key}")),
        "{}",
        issue.description
    );
    assert!(
        issue.description.contains("[user] 请检查构建日志"),
        "{}",
        issue.description
    );
    assert!(
        issue
            .description
            .contains("[assistant] 已检查，发现了 3 个警告"),
        "{}",
        issue.description
    );
    // 既有描述在前、注入块在后（单可溯源 + 原任务描述不丢）。
    let raw = issue.description.as_str();
    let at_desc = raw.find("原始描述").expect("原描述保留");
    let at_block = raw.find("父会话上下文").expect("注入块在");
    assert!(at_desc < at_block, "{}", raw);
    // 父会话存活 → 规则不暂停；二次触发照常。
    assert!(store.get_autopilot(ap.id).unwrap().enabled);
    let ap2 = store.get_autopilot(ap.id).unwrap();
    let out2 = fire_autopilot(&store, None, &ap2, &actor, None).unwrap();
    assert_eq!(out2["ran"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 不绑定 = 现行行为零变化：description 原样，无注入块。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn l23_unbound_rule_zero_change() {
    let dir = super::tests::unique_dir("l23-unbound");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let ap = store
        .create_autopilot(&new_ap("不绑定规则", None, true))
        .unwrap();
    let actor = nemesis_board::assignment::Actor::admin("admin");
    let out = fire_autopilot(&store, None, &ap, &actor, None).unwrap();
    let issue = store.get_issue(out["issue_id"].as_i64().unwrap()).unwrap();
    assert_eq!(issue.description, "原始描述");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 绑定但父会话已消失（从未物化 / clear 截空）→ fire Err（不建单）+
/// 规则自动停用（enabled=false 落库）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn l23_bound_rule_pauses_when_parent_missing_or_cleared() {
    let dir = super::tests::unique_dir("l23-pause-fire");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = nemesis_board::assignment::Actor::admin("admin");

    // ① 父会话从未物化（文件不存在）。
    let ap = store
        .create_autopilot(&new_ap(
            "幽灵绑定",
            Some("agent:main:session:l23ghost".to_string()),
            true,
        ))
        .unwrap();
    let err = fire_autopilot(&store, None, &ap, &actor, None).unwrap_err();
    assert!(err.contains("父会话"), "{err}");
    assert!(err.contains("已自动暂停"), "{err}");
    assert!(!store.get_autopilot(ap.id).unwrap().enabled, "规则已停用");
    assert!(
        store
            .list_issues_by_origin("autopilot", &ap.id.to_string(), 20)
            .unwrap()
            .is_empty(),
        "裁决失败不建单"
    );

    // ② 父会话 clear（截空文件仍在档）→ 同样裁决不存活。
    let key2 = "agent:main:session:l23cleared";
    nemesis_agent::chat_log::append_chat_log(key2, "user", "要被清空的内容");
    let ap2 = store
        .create_autopilot(&new_ap("清空绑定", Some(key2.to_string()), true))
        .unwrap();
    let _ = nemesis_agent::chat_log::clear_chat_log(key2);
    let err2 = fire_autopilot(&store, None, &ap2, &actor, None).unwrap_err();
    assert!(err2.contains("已自动暂停"), "{err2}");
    assert!(!store.get_autopilot(ap2.id).unwrap().enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

/// pause_autopilots_bound_to 直测：只暂停「绑定该会话且启用」的规则；
/// 停用规则与未绑定规则不受影响；⏸ 留痕评论落在最近一次 run 的单上。
#[tokio::test]
async fn l23_pause_autopilots_bound_to_cascades_and_leaves_trace() {
    let dir = super::tests::unique_dir("l23-cascade");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let key = "agent:main:session:l23cascade";
    nemesis_agent::chat_log::append_chat_log(key, "user", "上下文");

    // 规则 A：绑定 + 启用（从未运行 → ⏸ 留痕走「无评论面 = WARN 日志」臂）。
    let ap_a = store
        .create_autopilot(&new_ap("级联A", Some(key.to_string()), true))
        .unwrap();
    // 规则 B：绑定 + 启用，先造一张 run 单（验证 ⏸ 评论落最近 run 单）。
    let ap_b = store
        .create_autopilot(&new_ap("级联B", Some(key.to_string()), true))
        .unwrap();
    // 规则 C：绑定但停用（级联不动它——原本就停）。
    let ap_c = store
        .create_autopilot(&new_ap("级联C", Some(key.to_string()), false))
        .unwrap();
    // 规则 D：启用但不绑定（级联不动它）。
    let ap_d = store
        .create_autopilot(&new_ap("级联D", None, true))
        .unwrap();

    // B 的 run 单（直接 create_issue 挂 origin，不经 fire——fire 是
    // cluster 编译专属，本测试保持无 cluster 依赖）。
    let run_issue = store
        .create_issue(NewIssue {
            title: "历史运行单".to_string(),
            description: String::new(),
            acceptance_criteria: None,
            priority: priority::MEDIUM,
            project_id: None,
            creator: nemesis_board::assignment::Actor::admin("admin"),
            origin: Some(nemesis_board::models::TaskOrigin {
                origin_type: "autopilot".to_string(),
                origin_id: ap_b.id.to_string(),
                origin_meta: Default::default(),
            }),
            ..NewIssue::default()
        })
        .unwrap();

    let paused = pause_autopilots_bound_to(&store, key, "会话被删除");
    let mut expect = vec![ap_a.id, ap_b.id];
    expect.sort();
    assert_eq!(paused, expect, "只暂停绑定且启用的规则（A+B）");
    assert!(!store.get_autopilot(ap_a.id).unwrap().enabled);
    assert!(!store.get_autopilot(ap_b.id).unwrap().enabled);
    assert!(
        !store.get_autopilot(ap_c.id).unwrap().enabled,
        "C 原本就停用"
    );
    assert!(
        store.get_autopilot(ap_d.id).unwrap().enabled,
        "未绑定不受影响"
    );

    // ⏸ 留痕：评论落在 B 最近一次 run 的单上（A 无 run → 日志留痕臂）。
    let comments = store.list_comments(run_issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains('⏸') && c.content.contains("会话被删除")),
        "⏸ 留痕评论落最近 run 单: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// sessions.delete 级联（回收站软删）：绑定规则随响应 paused_autopilots
/// 汇报 + 停用落库；chat_log 三件套移走后存活判据归零。
#[tokio::test]
async fn l23_sessions_delete_cascades_pause_bound_autopilot() {
    let dir = super::tests::unique_dir("l23-del-cascade");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let sid = "l23del1";
    let key = format!("agent:main:session:{sid}");
    nemesis_agent::chat_log::append_chat_log(&key, "user", "将被删除的会话");
    let ap = store
        .create_autopilot(&new_ap("删除级联", Some(key.clone()), true))
        .unwrap();

    let out = ws(
        &ctx,
        "sessions",
        "delete",
        serde_json::json!({ "session_id": sid }),
    )
    .await
    .unwrap()
    .unwrap();
    let paused = out["paused_autopilots"].as_array().unwrap();
    assert!(
        paused.iter().any(|v| v.as_i64() == Some(ap.id)),
        "响应汇报被暂停规则: {out}"
    );
    assert!(!store.get_autopilot(ap.id).unwrap().enabled, "规则已停用");
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 1, None);
    assert_eq!(total, 0, "chat_log 已移入回收站 → 存活判据归零");
    let _ = std::fs::remove_dir_all(&dir);
}

/// sessions.clear 级联： 规格明确 clear 也在裁决范围（空会话继承零
/// 上下文 = 绑定无意义）——绑定规则自动暂停（不同于普通 cron job 的
/// 「clear 不级联」先例）。
#[tokio::test]
async fn l23_sessions_clear_cascades_pause_bound_autopilot() {
    let dir = super::tests::unique_dir("l23-clear-cascade");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let sid = "l23clr1";
    let key = format!("agent:main:session:{sid}");
    nemesis_agent::chat_log::append_chat_log(&key, "user", "将被清空的会话");
    let ap = store
        .create_autopilot(&new_ap("清空级联", Some(key.clone()), true))
        .unwrap();

    let out = ws(
        &ctx,
        "sessions",
        "clear",
        serde_json::json!({ "session_id": sid }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["cleared"], sid);
    assert!(
        !store.get_autopilot(ap.id).unwrap().enabled,
        "clear 后规则停用"
    );
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 1, None);
    assert_eq!(total, 0, "chat_log 已截空");
    let _ = std::fs::remove_dir_all(&dir);
}

/// WSAPI 绑定面 roundtrip：create 归一（trim + 空串 → None）、update
/// 三态（Some("")=清空 / Some(v)=覆盖 / 缺省=不改）。
#[tokio::test]
async fn l23_wsapi_autopilot_session_key_roundtrip() {
    let dir = super::tests::unique_dir("l23-wsapi");
    let _home = isolate(&dir);
    let ctx = super::tests::make_ctx_with_board(&dir);
    let store = store_of(&ctx);

    // create：带空白 binding → trim 归一。
    let out = ws(
        &ctx,
        "board",
        "autopilot.create",
        serde_json::json!({
            "name": "绑定面", "title": "t", "cron": "*/13 * * * *",
            "session_key": "  agent:main:session:l23ws  "
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let ap = out["autopilot"].clone();
    assert_eq!(
        ap["session_key"],
        serde_json::json!("agent:main:session:l23ws"),
        "create 归一 trim: {ap}"
    );
    let id = ap["id"].as_i64().unwrap();

    // create：空串 → 不绑定（null）。
    let out = ws(
        &ctx,
        "board",
        "autopilot.create",
        serde_json::json!({
            "name": "空绑定", "title": "t", "cron": "*/17 * * * *",
            "session_key": "   "
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out["autopilot"]["session_key"].is_null(),
        "空白 = 不绑定: {out}"
    );

    // update：Some("") = 解除绑定。
    let out = ws(
        &ctx,
        "board",
        "autopilot.update",
        serde_json::json!({ "id": id, "session_key": "" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out["autopilot"]["session_key"].is_null(),
        "空串 = 清空: {out}"
    );

    // update：Some(v) = 覆盖；缺省 = 不改。
    let out = ws(
        &ctx,
        "board",
        "autopilot.update",
        serde_json::json!({ "id": id, "session_key": "k2" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["autopilot"]["session_key"], serde_json::json!("k2"));
    let out = ws(
        &ctx,
        "board",
        "autopilot.update",
        serde_json::json!({ "id": id, "title": "t2" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        out["autopilot"]["session_key"],
        serde_json::json!("k2"),
        "缺省不改: {out}"
    );
    // store 直读复核（响应镜像 ≠ 真相源）。
    assert_eq!(
        store.get_autopilot(id).unwrap().session_key.as_deref(),
        Some("k2")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

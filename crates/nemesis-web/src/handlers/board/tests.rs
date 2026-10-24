//! Board handler dispatch 级测试（裸命令名经 `handle_cmd`，统一闸门硬性项）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_board::models::priority;
#[allow(unused_imports)]
use nemesis_config::DispatchFallbackMode;
use nemesis_types::cluster::NodeRole;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx_with_board(dir: &std::path::Path) -> RequestContext {
    make_ctx_with_role(dir, NodeRole::Coordinator)
}

/// 按节点角色构造带 board 服务的上下文（role 现为元数据；测试用 Worker/
/// Coordinator 两态钉「写权限与 role 无关」）。
fn make_ctx_with_role(dir: &std::path::Path, role: NodeRole) -> RequestContext {
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    make_ctx_with_service(dir, nemesis_board::BoardService::new(Arc::new(store), role))
}

/// 按给定 service 构造上下文（讨论桥等 builder 注入形态的测试入口）。
fn make_ctx_with_service(
    dir: &std::path::Path,
    service: nemesis_board::BoardService,
) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(dir.to_string_lossy().to_string()),
        home: Some(dir.to_string_lossy().to_string()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: Some(service),
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(dir.to_string_lossy().to_string()),
        home: Some(dir.to_string_lossy().to_string()),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn unique_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-web-boardtest-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn dispatch(
    ctx: &RequestContext,
    cmd: &str,
    data: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    BoardHandler.handle_cmd(cmd, Some(data), ctx).await
}

#[tokio::test]
async fn test_issue_create_list_get_flow() {
    let dir = unique_dir("crud");
    let ctx = make_ctx_with_board(&dir);

    // create
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "修复登录", "priority": priority::HIGH }),
    )
    .await
    .expect("create should succeed");
    let issue = out.unwrap()["issue"].clone();
    assert_eq!(issue["number"], "NB-1");
    assert_eq!(issue["status"], "backlog");
    let id = issue["id"].as_i64().unwrap();

    // list（含查询过滤）
    let out = dispatch(&ctx, "issue.list", serde_json::json!({ "query": "登录" }))
        .await
        .unwrap();
    assert_eq!(out.unwrap()["total"], 1);
    let out = dispatch(&ctx, "issue.list", serde_json::json!({ "query": "不存在" }))
        .await
        .unwrap();
    assert_eq!(out.unwrap()["total"], 0);

    // get by id / by number（带 comments/activity/subscribers 聚合）
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["id"], id);
    assert!(!out["issue"]["activity"].as_array().unwrap().is_empty());
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "number": "NB-1" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["number"], "NB-1");

    // get 缺参 → 报错
    assert!(
        dispatch(&ctx, "issue.get", serde_json::json!({}))
            .await
            .is_err()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_issue_create_anchor_warning_field() {
    // F5 件①：建单 AC 含 `[CHECK] re:` 锚点 → 响应带 warning（锚点下放
    // 纪律提醒）；file: 锚点/无锚点 → 无 warning（父单层 file: 可实核）。
    let dir = unique_dir("create-anchor-warn");
    let ctx = make_ctx_with_board(&dir);

    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({
            "title": "含锚点单",
            "acceptance_criteria": "功能可用\n[CHECK] re:交付完成",
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out["warning"].as_str().unwrap().contains("re: 锚点"),
        "含 re: 锚点建单必须带 warning: {out}"
    );

    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({
            "title": "file 锚点单",
            "acceptance_criteria": "[CHECK] file:docs/a.md exists",
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out.get("warning").is_none(),
        "file: 锚点不在警示范围: {out}"
    );

    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "无锚点单" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out.get("warning").is_none(),
        "无锚点建单不带 warning: {out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_project_create_anchor_warning_field() {
    // F5 件①：项目 AC 含 re: 锚点 → 响应带 warning（项目层无交付文本，
    // 锚点在项目收口评审时必然失败）；无锚点 → 无 warning。
    let dir = unique_dir("project-anchor-warn");
    let ctx = make_ctx_with_board(&dir);

    let out = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({
            "name": "锚点项目",
            "acceptance_criteria": "整体可用\n[CHECK] re:全部交付",
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out["warning"].as_str().unwrap().contains("项目"),
        "含 re: 锚点建项目必须带 warning: {out}"
    );

    let out = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "干净项目" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        out.get("warning").is_none(),
        "无锚点项目不带 warning: {out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_confirm_plan_anchor_downgrade_check_three_states() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    // F5 件③：父单 AC 含 re: 锚点但子单均未继承 → 父单系统评论提醒人工
    //（不阻塞发车）；子单已继承 / 父单无锚点 → 不评论。三态钉死。
    let dir = unique_dir("confirm-anchor-check");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let actor = nemesis_board::Actor::system("board");
    let mk_parent = |title: &str, ac: &str| {
        store
            .create_issue(nemesis_board::NewIssue {
                title: title.to_string(),
                acceptance_criteria: (!ac.is_empty()).then(|| ac.to_string()),
                creator: actor.clone(),
                ..nemesis_board::NewIssue::default()
            })
            .unwrap()
    };
    let sub = |title: &str, ac: &str| nemesis_board::PlannedSubIssue {
        title: title.to_string(),
        acceptance_criteria: ac.to_string(),
        ..nemesis_board::PlannedSubIssue::default()
    };
    let has_check_comment = |id: i64| -> bool {
        store
            .list_comments(id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("锚点下放检查"))
    };

    // ① 继承：父含 re: 锚点 + 一个子单继承 → 不提醒。
    let p1 = mk_parent("父甲", "整体可用\n[CHECK] re:全部交付");
    confirm_plan(
        &store,
        None, // 集群未装配 → 子单派发诚实降级（不撞本断言关注点）
        &p1,
        vec![
            sub("子甲1", "普通标准"),
            sub("子甲2", "[CHECK] re:子项交付"),
        ],
        &actor,
        &[],
    )
    .unwrap();
    assert!(!has_check_comment(p1.id), "子单已继承不应提醒: {p1:?}");

    // ② 未继承：父含 re: 锚点 + 子单全无 re: → 评论提醒且不阻塞发车
    //（file: 锚点不算继承——核验对象不同）。
    let p2 = mk_parent("父乙", "整体可用\n[CHECK] re:全部交付");
    let out = confirm_plan(
        &store,
        None,
        &p2,
        vec![
            sub("子乙1", "普通标准"),
            sub("子乙2", "[CHECK] file:docs/x.md exists"),
        ],
        &actor,
        &[],
    )
    .unwrap();
    assert_eq!(
        out["created"].as_array().unwrap().len(),
        2,
        "锚点检查绝不阻塞发车: {out}"
    );
    assert!(has_check_comment(p2.id), "子单未继承必须落提醒评论");

    // ③ 无锚点：父单 AC 无 re: 锚点 → 不评论。
    let p3 = mk_parent("父丙", "普通验收标准");
    confirm_plan(
        &store,
        None,
        &p3,
        vec![sub("子丙1", "普通标准")],
        &actor,
        &[],
    )
    .unwrap();
    assert!(!has_check_comment(p3.id), "父单无锚点不应提醒");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_issue_status_transition_and_illegal() {
    let dir = unique_dir("status");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "流转" }))
        .await
        .unwrap()
        .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    // 合法：backlog → in_progress
    let out = dispatch(
        &ctx,
        "issue.status",
        serde_json::json!({ "id": id, "status": "in_progress" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue"]["status"], "in_progress");
    // 转移写入 status_change 评论。
    assert!(
        out["issue"]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["ctype"] == "status_change")
    );

    // 非法：in_progress → backlog
    let err = dispatch(
        &ctx,
        "issue.status",
        serde_json::json!({ "id": id, "status": "backlog" }),
    )
    .await
    .expect_err("illegal transition must be rejected");
    assert!(err.contains("非法状态转移"), "{err}");

    // 未知 status 字符串
    let err = dispatch(
        &ctx,
        "issue.status",
        serde_json::json!({ "id": id, "status": "bogus" }),
    )
    .await
    .expect_err("unknown status must be rejected");
    assert!(err.contains("未知 status"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_issue_assign_and_update() {
    let dir = unique_dir("assign-update");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "派活" }))
        .await
        .unwrap()
        .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    // assign（成对提供）
    let out = dispatch(
        &ctx,
        "issue.assign",
        serde_json::json!({ "id": id, "assignee_type": "worker", "assignee_id": "node-b" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue"]["assignee"], "worker");
    assert_eq!(out["issue"]["assignee_id"], "node-b");

    // assign 只给一半 → 报错
    let err = dispatch(
        &ctx,
        "issue.assign",
        serde_json::json!({ "id": id, "assignee_type": "worker" }),
    )
    .await
    .expect_err("half assignee must be rejected");
    assert!(err.contains("成对"), "{err}");

    // update patch（priority + description）
    let out = dispatch(
        &ctx,
        "issue.update",
        serde_json::json!({ "id": id, "priority": priority::URGENT, "description": "新描述" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue"]["priority"], priority::URGENT);
    assert_eq!(out["issue"]["description"], "新描述");

    // update 缺 id → 报错
    assert!(
        dispatch(&ctx, "issue.update", serde_json::json!({ "title": "x" }))
            .await
            .is_err()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// 嵌套 patch 形态 loud 拒绝（gw36 实录回归锁）：`{id, patch:{...}}` 此前
// 静默不生效还回 updated:true——三个 update 命令都必须报错而不是假成功。
#[tokio::test]
async fn test_update_commands_reject_nested_patch_form() {
    let dir = unique_dir("nested-patch-reject");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "形态守卫" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    let err = dispatch(
        &ctx,
        "issue.update",
        serde_json::json!({ "id": id, "patch": { "title": "嵌套" } }),
    )
    .await
    .expect_err("nested patch form must be rejected");
    assert!(err.contains("嵌套 patch"), "{err}");
    // 确认拒绝发生在写库前：title 未被改动。
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["title"], "形态守卫");

    let err = dispatch(
        &ctx,
        "autopilot.update",
        serde_json::json!({ "id": 1, "patch": { "enabled": true } }),
    )
    .await
    .expect_err("autopilot.update nested form must be rejected");
    assert!(err.contains("嵌套 patch"), "{err}");

    let err = dispatch(
        &ctx,
        "project.update",
        serde_json::json!({ "id": 1, "patch": { "name": "x" } }),
    )
    .await
    .expect_err("project.update nested form must be rejected");
    assert!(err.contains("嵌套 patch"), "{err}");

    // 非对象的 patch 字段（null）不算嵌套形态，不误伤扁平更新。
    dispatch(
        &ctx,
        "issue.update",
        serde_json::json!({ "id": id, "description": "ok", "patch": null }),
    )
    .await
    .unwrap()
    .unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_comments_activity_subscribers() {
    let dir = unique_dir("social");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "讨论" }))
        .await
        .unwrap()
        .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    dispatch(
        &ctx,
        "comment.add",
        serde_json::json!({ "issue_id": id, "content": "第一条" }),
    )
    .await
    .expect("comment.add should succeed");
    // 空评论被拒（store 校验透传）。
    assert!(
        dispatch(
            &ctx,
            "comment.add",
            serde_json::json!({ "issue_id": id, "content": "  " })
        )
        .await
        .is_err()
    );

    let out = dispatch(&ctx, "comment.list", serde_json::json!({ "issue_id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["comments"].as_array().unwrap().len(), 1);

    let out = dispatch(&ctx, "activity.list", serde_json::json!({ "issue_id": id }))
        .await
        .unwrap()
        .unwrap();
    let acts = out["activity"].as_array().unwrap();
    assert!(acts.iter().any(|a| a["action"] == "created"));
    assert!(acts.iter().any(|a| a["action"] == "commented"));

    dispatch(
        &ctx,
        "subscriber.add",
        serde_json::json!({ "issue_id": id }),
    )
    .await
    .expect("subscriber.add should succeed");
    let out = dispatch(
        &ctx,
        "subscriber.list",
        serde_json::json!({ "issue_id": id }),
    )
    .await
    .unwrap()
    .unwrap();
    let subs = out["subscribers"].as_array().unwrap();
    // 创建者（admin/dashboard，固定身份）+ 手动订阅（同一 admin 身份 → 幂等一条）。
    assert!(subs.iter().any(|s| s["subscriber"]["kind"] == "admin"));
    dispatch(
        &ctx,
        "subscriber.remove",
        serde_json::json!({ "issue_id": id }),
    )
    .await
    .expect("subscriber.remove should succeed");
    let out = dispatch(
        &ctx,
        "subscriber.list",
        serde_json::json!({ "issue_id": id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(out["subscribers"].as_array().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_projects_attachments_stats() {
    let dir = unique_dir("proj");
    let ctx = make_ctx_with_board(&dir);

    let out = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "主项目", "icon": "🚀" }),
    )
    .await
    .unwrap()
    .unwrap();
    let pid = out["project"]["id"].as_i64().unwrap();
    // 重名拒绝。
    assert!(
        dispatch(
            &ctx,
            "project.create",
            serde_json::json!({ "name": "主项目" })
        )
        .await
        .is_err()
    );
    let out = dispatch(&ctx, "project.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["projects"].as_array().unwrap().len(), 1);

    // project 过滤 + stats + attachment。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "带项目", "project_id": pid }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();
    let out = dispatch(&ctx, "issue.list", serde_json::json!({ "project_id": pid }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1);

    let out = dispatch(&ctx, "stats", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["by_status"]["backlog"], 1);

    // attachment.list 空（P1 只读元数据）。
    let out = dispatch(
        &ctx,
        "attachment.list",
        serde_json::json!({ "issue_id": id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["attachments"].as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

/// 角色门控已移除（2026-08-31 修复）：board.db 是节点本地数据（无集群同步、
/// CLI 同权直写），worker 对本机看板有完整写权——完整 CRUD 往返必须成功。
#[tokio::test]
async fn test_worker_role_has_full_write_access() {
    let dir = unique_dir("worker-write");
    let ctx = make_ctx_with_role(&dir, NodeRole::Worker);

    // worker 完整 CRUD 往返。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "worker 写入", "priority": 2 }),
    )
    .await
    .expect("worker must create issues locally");
    let issue = &out.unwrap()["issue"];
    assert_eq!(issue["number"], "NB-1");
    let id = issue["id"].as_i64().unwrap();

    let out = dispatch(
        &ctx,
        "issue.update",
        serde_json::json!({ "id": id, "priority": 3 }),
    )
    .await
    .expect("worker must update issues locally");
    assert_eq!(out.unwrap()["issue"]["priority"], 3);

    dispatch(
        &ctx,
        "comment.add",
        serde_json::json!({ "issue_id": id, "content": "来自 worker" }),
    )
    .await
    .expect("worker must add comments locally");

    let out = dispatch(
        &ctx,
        "issue.status",
        serde_json::json!({ "id": id, "status": "todo" }),
    )
    .await
    .expect("worker must transition status locally");
    assert_eq!(out.unwrap()["issue"]["status"], "todo");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 回归钉：曾经的 role 403 门控不得回归——18 个写命令在 worker 节点上允许
/// 进入参数校验阶段（有错也只能是业务校验错，绝不允许再出现 "403:" 语义）。
#[tokio::test]
async fn test_worker_role_never_403() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("worker-no-403");
    let ctx = make_ctx_with_role(&dir, NodeRole::Worker);

    for (cmd, data) in [
        ("issue.create", serde_json::json!({ "title": "x" })),
        ("issue.update", serde_json::json!({ "id": 1, "title": "x" })),
        ("issue.assign", serde_json::json!({ "id": 1 })),
        (
            "issue.status",
            serde_json::json!({ "id": 1, "status": "todo" }),
        ),
        (
            "issue.move",
            serde_json::json!({ "id": 1, "status": "todo", "position": 0 }),
        ),
        ("issue.dispatch", serde_json::json!({ "id": 1 })),
        ("issue.cancel", serde_json::json!({ "id": 1 })),
        (
            "comment.add",
            serde_json::json!({ "issue_id": 1, "content": "x" }),
        ),
        ("subscriber.add", serde_json::json!({ "issue_id": 1 })),
        ("subscriber.remove", serde_json::json!({ "issue_id": 1 })),
        ("project.create", serde_json::json!({ "name": "p" })),
        ("project.update", serde_json::json!({ "id": 1 })),
        (
            "attachment.add",
            serde_json::json!({ "issue_id": 1, "filename": "a.txt", "content": "eA==" }),
        ),
        ("inbox.mark_read", serde_json::json!({ "all": true })),
        (
            "autopilot.create",
            serde_json::json!({ "name": "ap", "title": "t", "cron": "0 9 * * *" }),
        ),
        ("autopilot.update", serde_json::json!({ "id": 1 })),
        ("autopilot.remove", serde_json::json!({ "id": 1 })),
        ("autopilot.run", serde_json::json!({ "id": 1 })),
    ] {
        if let Err(err) = dispatch(&ctx, cmd, data).await {
            assert!(
                !err.starts_with("403:"),
                "{cmd} must not be role-gated on worker: {err}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// coordinator 写路径照常可用（role 现在只是元数据，不改变任何权限）。
#[tokio::test]
async fn test_coordinator_role_writes_allowed() {
    let dir = unique_dir("coordinator-gate");
    let ctx = make_ctx_with_role(&dir, NodeRole::Coordinator);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "权威节点" }),
    )
    .await
    .expect("coordinator writes must pass the gate");
    assert_eq!(out.unwrap()["issue"]["number"], "NB-1");
    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P2 派发链路（无集群实例的校验矩阵）：本地校验先行（目标/状态/重复
/// 派发），集群缺失最后报——错误指向明确且校验不依赖集群实例。
/// （dispatch 走集群，无 cluster feature 时 handler 直接报缺依赖 → 测试随之门控。）
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_issue_dispatch_validation_without_cluster() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("dispatch-validation");
    let ctx = make_ctx_with_board(&dir);

    // 缺 id。
    let err = dispatch(&ctx, "issue.dispatch", serde_json::json!({}))
        .await
        .expect_err("missing id must error");
    assert!(err.contains("missing field: id"), "{err}");

    // 建 issue（无指派）→ 缺目标。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "派发目标校验" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();
    let err = dispatch(&ctx, "issue.dispatch", serde_json::json!({ "id": id }))
        .await
        .expect_err("no target must error");
    assert!(err.contains("缺少派发目标"), "{err}");

    // manager_self 指派 → 明确拒绝远端派发。
    dispatch(
        &ctx,
        "issue.assign",
        serde_json::json!({ "id": id, "assignee_type": "manager_self", "assignee_id": "coord-1" }),
    )
    .await
    .unwrap();
    let err = dispatch(&ctx, "issue.dispatch", serde_json::json!({ "id": id }))
        .await
        .expect_err("manager_self must be rejected");
    assert!(err.contains("manager_self"), "{err}");

    // 显式 target 覆盖指派 → 校验全过后，集群缺失收尾。
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": id, "target": "node-b" }),
    )
    .await
    .expect_err("missing cluster must error last");
    assert!(err.contains("集群未运行"), "{err}");

    // 终态（done）→ 不可派发。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "终态派发" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id2 = out["issue"]["id"].as_i64().unwrap();
    dispatch(
        &ctx,
        "issue.status",
        serde_json::json!({ "id": id2, "status": "done" }),
    )
    .await
    .unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": id2, "target": "node-b" }),
    )
    .await
    .expect_err("terminal issue must be rejected");
    assert!(err.contains("不可派发"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// F-U4-5（2026-09-15 真机实证）：急停发车护栏——estop ENGAGED 时手动
/// 发车面（issue.dispatch / issue.plan / autopilot.run / project.create
/// auto_start）一律诚实拒绝；release 后放行到既有校验链（此处无集群，
/// 落到「集群未运行」即证明护栏已过）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_estop_blocks_dispatch_family_entries() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("estop-dispatch-gate");
    let ctx = make_ctx_with_board(&dir);

    // 建 issue（建单不是发车面，急停中放行）。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "急停护栏" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    // 挂急停（ctx.state.estop 置为 ENGAGED 态——AppState: Clone 字段换新）。
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    assert!(estop.is_engaged());
    let mut estopped = (*ctx.state).clone();
    estopped.estop = Some(estop);
    let ctx_e = RequestContext {
        session_id: ctx.session_id.clone(),
        chat_id: ctx.chat_id.clone(),
        workspace: ctx.workspace.clone(),
        home: ctx.home.clone(),
        state: Arc::new(estopped),
        auth_method: ctx.auth_method.clone(),
    };

    // issue.dispatch → 拒（护栏先于一切校验，连 data 都不解析）。
    let err = dispatch(&ctx_e, "issue.dispatch", serde_json::json!({}))
        .await
        .expect_err("estop must refuse dispatch");
    assert!(err.contains("急停（E-STOP）"), "{err}");

    // issue.plan → 拒。
    let err = dispatch(&ctx_e, "issue.plan", serde_json::json!({}))
        .await
        .expect_err("estop must refuse plan");
    assert!(err.contains("急停（E-STOP）"), "{err}");

    // autopilot.run → 拒（护栏在 id 解析前）。
    let err = dispatch(&ctx_e, "autopilot.run", serde_json::json!({}))
        .await
        .expect_err("estop must refuse autopilot run");
    assert!(err.contains("急停（E-STOP）"), "{err}");

    // project.create + auto_start → 项目照建（非发车面），auto_start 拒。
    let out = dispatch(
        &ctx_e,
        "project.create",
        serde_json::json!({ "name": "急停项目", "auto_start": true }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["created"], true, "project itself must still be created");
    let err = out["auto_start"]["error"].as_str().unwrap_or("");
    assert!(
        err.contains("急停（E-STOP）"),
        "auto_start must be refused: {out}"
    );

    // 释放 → 同一批入口放行到既有校验链（无集群 → 「集群未运行」/
    // missing field，均证明已过护栏）。
    ctx_e.state.estop.as_ref().unwrap().release();
    let err = dispatch(
        &ctx_e,
        "issue.dispatch",
        serde_json::json!({ "id": id, "target": "node-b" }),
    )
    .await
    .expect_err("after release must fall through to cluster check");
    assert!(err.contains("集群未运行"), "{err}");

    let err = dispatch(&ctx_e, "issue.plan", serde_json::json!({}))
        .await
        .expect_err("after release plan must reach its own validation");
    assert!(err.contains("missing field: id"), "{err}");

    let err = dispatch(&ctx_e, "autopilot.run", serde_json::json!({}))
        .await
        .expect_err("after release autopilot.run must reach id validation");
    assert!(err.contains("missing field: id"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_board_not_injected_and_unknown_cmd() {
    let dir = unique_dir("noinject");
    let ctx = make_ctx_with_board(&dir);
    // 摘掉 board 注入 → 统一报 "board service not available"。
    let mut ctx2 = RequestContext {
        session_id: ctx.session_id.clone(),
        chat_id: ctx.chat_id.clone(),
        workspace: ctx.workspace.clone(),
        home: ctx.home.clone(),
        state: ctx.state.clone(),
        auth_method: crate::session::AuthMethod::default(),
    };
    // 复制 state 后替换 board 字段：直接构造一个 board=None 的 AppState 克隆。
    ctx2.state = {
        let mut s = (*ctx.state).clone();
        s.board = None;
        Arc::new(s)
    };
    let err = BoardHandler
        .handle_cmd("issue.list", Some(serde_json::json!({})), &ctx2)
        .await
        .expect_err("missing board must error");
    assert_eq!(err, "board service not available");

    // 未知命令。
    let err = BoardHandler
        .handle_cmd("bogus", Some(serde_json::json!({})), &ctx)
        .await
        .expect_err("unknown command must error");
    assert!(err.contains("unknown command: board.bogus"), "{err}");

    // 缺 workspace。
    let mut ctx3 = RequestContext {
        session_id: ctx.session_id.clone(),
        chat_id: ctx.chat_id.clone(),
        workspace: None,
        home: ctx.home.clone(),
        state: ctx.state.clone(),
        auth_method: crate::session::AuthMethod::default(),
    };
    ctx3.state = ctx.state.clone();
    let err = BoardHandler
        .handle_cmd("issue.list", Some(serde_json::json!({})), &ctx3)
        .await
        .expect_err("missing workspace must error");
    assert_eq!(err, "workspace not configured");

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P3 看板拖拽：同列重排只改 position（无 status_change 评论，产生
/// reordered 活动）；跨列走状态机（status_change 评论）；非法转移与缺参拒绝。
#[tokio::test]
async fn test_issue_move_handler() {
    let dir = unique_dir("move");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "拖拽" }))
        .await
        .unwrap()
        .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    // 同列重排：只改 position，不产生 status_change 评论。
    let out = dispatch(
        &ctx,
        "issue.move",
        serde_json::json!({ "id": id, "status": "backlog", "position": 42 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["moved"], true);
    assert_eq!(out["issue"]["status"], "backlog");
    assert_eq!(out["issue"]["position"], 42);
    assert!(
        out["issue"]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["ctype"] != "status_change")
    );
    let out = dispatch(&ctx, "activity.list", serde_json::json!({ "issue_id": id }))
        .await
        .unwrap()
        .unwrap();
    assert!(
        out["activity"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["action"] == "reordered")
    );

    // 跨列：backlog → in_progress + 指定 position（一个原子操作）。
    let out = dispatch(
        &ctx,
        "issue.move",
        serde_json::json!({ "id": id, "status": "in_progress", "position": 7 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue"]["status"], "in_progress");
    assert_eq!(out["issue"]["position"], 7);
    assert!(
        out["issue"]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["ctype"] == "status_change")
    );

    // 非法转移（in_progress → backlog）被状态机拒绝。
    let err = dispatch(
        &ctx,
        "issue.move",
        serde_json::json!({ "id": id, "status": "backlog", "position": 0 }),
    )
    .await
    .expect_err("illegal move must be rejected");
    assert!(err.contains("非法状态转移"), "{err}");

    // 缺 position。
    let err = dispatch(
        &ctx,
        "issue.move",
        serde_json::json!({ "id": id, "status": "todo" }),
    )
    .await
    .expect_err("missing position must error");
    assert!(err.contains("missing field: position"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P3 附件上传/下载：base64 内容落盘 workspace/board/files/issue_N/，
/// storage_path 记 workspace 相对路径；坏 base64 / 穿越文件名 / 缺 issue 拒绝。
#[tokio::test]
async fn test_attachment_add_get_roundtrip() {
    let dir = unique_dir("attachment");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "带附件" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    // 上传 "hello"（base64）→ 落盘 + 元数据入表。
    let out = dispatch(
        &ctx,
        "attachment.add",
        serde_json::json!({ "issue_id": id, "filename": "note.txt", "content": "aGVsbG8=" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["added"], true);
    assert_eq!(out["attachment"]["filename"], "note.txt");
    assert_eq!(out["attachment"]["size"], 5);
    let att_id = out["attachment"]["id"].as_i64().unwrap();
    let storage_path = out["attachment"]["storage_path"].as_str().unwrap();
    assert!(
        storage_path.starts_with("board/files/issue_"),
        "{storage_path}"
    );
    let files_dir = dir.join("board").join("files").join(format!("issue_{id}"));
    assert_eq!(
        std::fs::read_dir(&files_dir).unwrap().count(),
        1,
        "exactly one stored file"
    );

    // 下载回读 → base64 解码回原文。
    let out = dispatch(&ctx, "attachment.get", serde_json::json!({ "id": att_id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["content"], "aGVsbG8=");
    assert_eq!(out["attachment"]["id"], att_id);

    // attachment.list 元数据可见。
    let out = dispatch(
        &ctx,
        "attachment.list",
        serde_json::json!({ "issue_id": id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["attachments"].as_array().unwrap().len(), 1);

    // 坏 base64 拒绝。
    let err = dispatch(
        &ctx,
        "attachment.add",
        serde_json::json!({ "issue_id": id, "filename": "x.txt", "content": "!!!" }),
    )
    .await
    .expect_err("bad base64 must be rejected");
    assert!(err.contains("base64"), "{err}");

    // `..` 文件名拒绝（穿越段会被消毒取基本名，`..` 本体无基本名）。
    let err = dispatch(
        &ctx,
        "attachment.add",
        serde_json::json!({ "issue_id": id, "filename": "..", "content": "aGVsbG8=" }),
    )
    .await
    .expect_err("dotdot filename must be rejected");
    assert!(err.contains("非法附件文件名"), "{err}");

    // 不存在的 issue 拒绝（先校验后落盘 → 不产生新文件）。
    assert!(
        dispatch(
            &ctx,
            "attachment.add",
            serde_json::json!({ "issue_id": 99999, "filename": "x.txt", "content": "aGVsbG8=" }),
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read_dir(&files_dir).unwrap().count(),
        1,
        "no file for missing issue"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P3 项目字段级更新：patch 语义（未提供字段保持原值）、空名拒绝、
/// 缺 id / 不存在的项目拒绝。
#[tokio::test]
async fn test_project_update_handler() {
    let dir = unique_dir("project-update");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "旧名", "description": "旧描述" }),
    )
    .await
    .unwrap()
    .unwrap();
    let pid = out["project"]["id"].as_i64().unwrap();

    // 字段级 patch：改名 + 归档（未提供的 description 保持原值）。
    let out = dispatch(
        &ctx,
        "project.update",
        serde_json::json!({ "id": pid, "name": "新名", "status": "archived" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(out["project"]["name"], "新名");
    assert_eq!(out["project"]["status"], "archived");
    assert_eq!(out["project"]["description"], "旧描述");

    // 空名拒绝。
    let err = dispatch(
        &ctx,
        "project.update",
        serde_json::json!({ "id": pid, "name": "  " }),
    )
    .await
    .expect_err("empty name must be rejected");
    assert!(err.contains("must not be empty"), "{err}");

    // 缺 id。
    assert!(
        dispatch(&ctx, "project.update", serde_json::json!({ "name": "x" }))
            .await
            .is_err()
    );

    // 不存在的项目。
    let err = dispatch(
        &ctx,
        "project.update",
        serde_json::json!({ "id": 99999, "name": "x" }),
    )
    .await
    .expect_err("missing project must error");
    assert!(err.contains("not found"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P3 收件箱：store 事件钩子产生通知（测试经 store 直播种子，handler
/// 评论作者=创建者会被排除）；admin wildcard 全量可见；单条/全部已读幂等。
#[tokio::test]
async fn test_inbox_list_and_mark_read() {
    let dir = unique_dir("inbox");
    let ctx = make_ctx_with_board(&dir);

    // 直接经 store 播种两条 admin 通知（comment.add 的作者即创建者本人，
    // 通知会把作者排除，不适合造数）。
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    for title in ["通知一", "通知二"] {
        store
            .notify(nemesis_board::NewNotification {
                recipient: nemesis_board::Actor::admin("admin"),
                kind: nemesis_board::notification_kind::COMMENTED.to_string(),
                title: title.to_string(),
                content: "正文".to_string(),
                issue_id: None,
            })
            .expect("seed notification");
    }

    // inbox.list → admin wildcard（recipient_id=None）收全量，最新在前。
    let out = dispatch(&ctx, "inbox.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["unread"], 2);
    let list = out["notifications"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["title"], "通知二");

    // unread_only + limit 过滤。
    let out = dispatch(
        &ctx,
        "inbox.list",
        serde_json::json!({ "unread_only": true, "limit": 1 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["notifications"].as_array().unwrap().len(), 1);

    // 单条已读（幂等：第二次 marked=0）。
    let first_id = list[0]["id"].as_i64().unwrap();
    let out = dispatch(
        &ctx,
        "inbox.mark_read",
        serde_json::json!({ "id": first_id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["marked"], 1);
    assert_eq!(out["unread"], 1);
    let out = dispatch(
        &ctx,
        "inbox.mark_read",
        serde_json::json!({ "id": first_id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["marked"], 0);
    assert_eq!(out["unread"], 1);

    // 全部已读。
    let out = dispatch(&ctx, "inbox.mark_read", serde_json::json!({ "all": true }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["marked"], 1);
    assert_eq!(out["unread"], 0);
    let out = dispatch(
        &ctx,
        "inbox.list",
        serde_json::json!({ "unread_only": true }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["notifications"].as_array().unwrap().len(), 0);

    // 缺 id 且无 all → 报错。
    assert!(
        dispatch(&ctx, "inbox.mark_read", serde_json::json!({}))
            .await
            .is_err()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P4 issue.cancel 校验矩阵（无集群实例，B3 修订后的契约）：无派发
/// 直接终态（停车场单/依赖闸未放行单不需要集群）；有在途派发时集群缺失
/// 在动账前拒（取消 = 下行 task_cancel，竞态守卫在集群校验之后才动账）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_issue_cancel_validation_without_cluster() {
    let dir = unique_dir("cancel-validation");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();

    // 建 issue → 无派发 → 直接终态（B3：不再要求集群参与）。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "待取消" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();
    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], true, "无派发单必须可直接取消");
    assert!(out["task_id"].is_null(), "无派发取消不得编造 task_id");
    assert_eq!(out["issue"]["status"], "cancelled");

    // 已终态单再取消：幂等（状态机原地不动，联动早退）。
    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["status"], "cancelled");

    // 直播一条 dispatched 派发（绕过集群）→ 有在途派发时集群必需。
    let other = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "在途取消" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id2 = other["issue"]["id"].as_i64().unwrap();
    let actor = nemesis_board::Actor::admin("test-session");
    store
        .insert_dispatch("task-cancel-1", id2, "node-b", &actor)
        .expect("seed dispatch");
    let err = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": id2 }))
        .await
        .expect_err("missing cluster must error");
    assert!(err.contains("集群未运行"), "{err}");

    // 未动账：派发仍在、issue 状态未变。
    assert!(store.get_active_dispatch(id2).unwrap().is_some());
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "id": id2 }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["status"], "backlog");

    let _ = std::fs::remove_dir_all(&dir);
}

/// B14 issue.cancel_dispatch（只撤派发、单据存活）：无在途派发诚实报错；
/// 有在途派发、集群缺失 = 本地撤回 + 下行退化 warn（撤回比 issue.cancel
/// 轻、可逆——重派即可，无需集群在动账前把关）；撤后 issue 状态不动、
/// 活动派发清零（重派闸放行）、再撤诚实报错、留痕评论在场。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_issue_cancel_dispatch_keeps_issue_alive() {
    let dir = unique_dir("cancel-dispatch");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();

    // 无在途派发 → 诚实报错。
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "撤派发" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();
    let err = dispatch(
        &ctx,
        "issue.cancel_dispatch",
        serde_json::json!({ "id": id }),
    )
    .await
    .expect_err("no active dispatch must error");
    assert!(err.contains("无在途派发"), "{err}");

    // 直播一条 dispatched 派发（绕过集群）→ 撤回成功（集群缺失 = 仅本地
    // 撤回，下行 warn 不炸本体）。
    let actor = nemesis_board::Actor::admin("test-session");
    store
        .insert_dispatch("task-cd-1", id, "node-b", &actor)
        .expect("seed dispatch");
    let out = dispatch(
        &ctx,
        "issue.cancel_dispatch",
        serde_json::json!({ "id": id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["cancelled_dispatch"], true, "撤回应答必须立旗");
    assert_eq!(out["task_id"], "task-cd-1");

    // issue 状态不动（可直接重派）+ 活动派发清零（重派闸放行）。
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["status"], "backlog", "撤派发不得动 issue 状态");
    assert!(
        !store.has_active_dispatch(id).unwrap(),
        "撤后重派闸必须放行"
    );

    // 派发行已终态：再撤 = 诚实报错（active 查不到 → 无在途派发文案）。
    let err = dispatch(
        &ctx,
        "issue.cancel_dispatch",
        serde_json::json!({ "id": id }),
    )
    .await
    .expect_err("terminal dispatch must error");
    assert!(err.contains("无在途派发"), "{err}");

    // 留痕评论在场（撤回不改状态，评论是重派线索的唯一载体）。
    let comments = store.list_comments(id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("派发已撤回")),
        "撤回必须留痕"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P4 autopilot：CRUD + 手动触发（target 空 → 仅建单，`{date}` 替换，
/// origin=autopilot 进 run 历史）+ target 非空但集群缺失 → 建单前拒绝 +
/// remove。cron 未注入（AppState.cron=None）→ 跳过 job 登记，cron_job_id
/// 保持 None（gateway 启动同步兜底）。
#[tokio::test]
async fn test_autopilot_crud_manual_run_and_history() {
    let dir = unique_dir("autopilot");
    let ctx = make_ctx_with_board(&dir);

    // create：坏 cron 先拒。
    let err = dispatch(
        &ctx,
        "autopilot.create",
        serde_json::json!({ "name": "坏", "title": "t", "cron": "not-a-cron" }),
    )
    .await
    .expect_err("bad cron must be rejected");
    assert!(err.contains("invalid cron"), "{err}");

    // create（target 空）→ cron 未注入也能建，cron_job_id 为 null。
    let out = dispatch(
        &ctx,
        "autopilot.create",
        serde_json::json!({
            "name": "每日报表", "title": "日报 {date}", "cron": "0 9 * * *", "target": ""
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["created"], true);
    let ap = out["autopilot"].clone();
    let ap_id = ap["id"].as_i64().unwrap();
    assert_eq!(ap["enabled"], true);
    assert!(ap["cron_job_id"].is_null());
    assert_eq!(ap["title"], "日报 {date}");

    // create（target 非空，用于派发拒绝分支）。
    let out = dispatch(
        &ctx,
        "autopilot.create",
        serde_json::json!({
            "name": "派活", "title": "任务 {date}", "cron": "0 10 * * *", "target": "node-b"
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let ap_dispatch_id = out["autopilot"]["id"].as_i64().unwrap();

    // list。
    let out = dispatch(&ctx, "autopilot.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["autopilots"].as_array().unwrap().len(), 2);

    // update：patch 语义（只改 enabled，其余不动）+ 坏 cron 拒绝。
    let out = dispatch(
        &ctx,
        "autopilot.update",
        serde_json::json!({ "id": ap_id, "enabled": false }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(out["autopilot"]["enabled"], false);
    assert_eq!(out["autopilot"]["title"], "日报 {date}");
    assert!(
        dispatch(
            &ctx,
            "autopilot.update",
            serde_json::json!({ "id": ap_id, "cron": "bad" })
        )
        .await
        .is_err()
    );

    // 手动 run（target 空、enabled=false 也可手动触发）：仅建单 + {date} 替换。
    let out = dispatch(&ctx, "autopilot.run", serde_json::json!({ "id": ap_id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["ran"], true);
    assert!(out["dispatch"].is_null());
    let number = out["issue_number"].as_str().unwrap().to_string();
    assert!(
        !number.contains("{date}"),
        "placeholder must be substituted: {number}"
    );

    // target 非空 + 集群缺失 → 建单前拒绝（不留半成品）。拒绝臂随编译形态：
    // cluster 编译但未运行（nemesisbot 全量常态）→「集群未运行」；cluster
    // 未编译（minimal-iot 裁剪档 / nightly feature-matrix 同形态）→
    // 「cluster feature 未编译」。两者都是正确生产行为，按实际编译形态断言
    // 对应臂（2026-09-02 CI 实录：本测试未像 dispatch/cancel 测试那样整体
    // 门控，在无 cluster 编译下钉死单臂假红）。
    let err = dispatch(
        &ctx,
        "autopilot.run",
        serde_json::json!({ "id": ap_dispatch_id }),
    )
    .await
    .expect_err("dispatch target without cluster must error");
    let expected = if cfg!(feature = "cluster") {
        "集群未运行"
    } else {
        "cluster feature 未编译"
    };
    assert!(err.contains(expected), "{err}");

    // run 历史：origin=autopilot 的 issue（只有 target 空的那次）。
    let out = dispatch(&ctx, "autopilot.runs", serde_json::json!({ "id": ap_id }))
        .await
        .unwrap()
        .unwrap();
    let issues = out["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["origin"]["origin_type"], "autopilot");
    assert_eq!(issues[0]["origin"]["origin_id"], ap_id.to_string());
    // last_run_at 已落账。
    let out = dispatch(&ctx, "autopilot.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let aps = out["autopilots"].as_array().unwrap();
    let ap_view = aps.iter().find(|a| a["id"] == ap_id).unwrap();
    assert!(!ap_view["last_run_at"].is_null());

    // remove → 再 run 报 not found。
    let out = dispatch(&ctx, "autopilot.remove", serde_json::json!({ "id": ap_id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["removed"], true);
    assert!(
        dispatch(&ctx, "autopilot.run", serde_json::json!({ "id": ap_id }))
            .await
            .is_err()
    );

    // 缺 id。
    assert!(
        dispatch(
            &ctx,
            "autopilot.update",
            serde_json::json!({ "enabled": true })
        )
        .await
        .is_err()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// W2.5 自动派发接口（board.auto_dispatch，默认关；用户拍板 2026-08-31）
// ---------------------------------------------------------------------------

#[test]
fn auto_dispatch_gate_requires_switch_and_worker() {
    // 无 board 段（load_live None 的判定输入等价）→ 恒关。
    assert!(!should_auto_dispatch(None, Some(AssignmentType::Worker)));
    // 有段但开关关（Default）→ 关。
    let off = nemesis_config::BoardFlagConfig::default();
    assert!(!off.auto_dispatch);
    assert!(!should_auto_dispatch(
        Some(&off),
        Some(AssignmentType::Worker)
    ));
    // 开关开 + worker 指派 → 触发。
    let on = nemesis_config::BoardFlagConfig {
        auto_dispatch: true,
        ..Default::default()
    };
    assert!(should_auto_dispatch(
        Some(&on),
        Some(AssignmentType::Worker)
    ));
    // 开关开但非 worker（manager_self / 未指派）→ 不触发。
    assert!(!should_auto_dispatch(
        Some(&on),
        Some(AssignmentType::ManagerSelf)
    ));
    assert!(!should_auto_dispatch(Some(&on), None));
}

#[tokio::test]
async fn assign_worker_default_stays_pending_no_dispatch() {
    // 默认（无全局 config store → live_board_config None）：指派给 worker
    // 只写 assignee 元数据，不触发派发——行为与 W2.5 之前完全一致
    //（不要求集群、状态不推进、无 ⛔ 评论）。
    let dir = unique_dir("assign-default-off");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "默认关" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();

    let out = dispatch(
        &ctx,
        "issue.assign",
        serde_json::json!({ "id": id, "assignee_type": "worker", "assignee_id": "node-b" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["assigned"], true);
    assert_eq!(out["issue"]["assignee"], "worker");
    assert_eq!(out["issue"]["status"], "backlog");

    // 无系统评论（⛔ 自动派发失败等）产生。
    let out = dispatch(&ctx, "comment.list", serde_json::json!({ "issue_id": id }))
        .await
        .unwrap()
        .unwrap();
    assert!(out["comments"].as_array().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn auto_dispatch_on_without_cluster_leaves_trace_comment() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    // 开关开 + 无集群：内核函数直接调用（config 显式入参，不碰进程级全局
    // config store——OnceLock 不可清除，全局态操作是并行 flake 源）。断言
    // ① 返回 false（未派发）② ⛔ 系统评论留痕（「配置生效但能力缺失」
    // 可观测，与 issue.assign handler 内的调用同语义）。
    let dir = unique_dir("auto-on-nocluster");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let actor = nemesis_board::Actor::admin("test-session");

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "开关开无集群".into(),
            ..Default::default()
        })
        .unwrap();
    let issue = store
        .assign_issue(
            issue.id,
            Some(AssignmentType::Worker),
            Some("node-b".to_string()),
            &actor,
        )
        .unwrap();

    // 开关开 + 无集群 → 派发失败留痕，返回 false。
    let on = nemesis_config::BoardFlagConfig {
        auto_dispatch: true,
        ..Default::default()
    };
    let dispatched = super::auto_dispatch_with_config(Some(&on), &store, None, &issue, &actor);
    assert!(!dispatched, "no cluster → dispatch must not succeed");

    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| {
            matches!(c.ctype, nemesis_board::CommentType::System)
                && c.content.contains("自动派发失败")
        }),
        "expected auto-dispatch failure trace comment, got {comments:?}"
    );

    // 开关关（默认）→ 同一调用零评论零派发（gate 短路，无副作用）。
    let issue2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "开关关".into(),
            ..Default::default()
        })
        .unwrap();
    let issue2 = store
        .assign_issue(
            issue2.id,
            Some(AssignmentType::Worker),
            Some("node-b".to_string()),
            &actor,
        )
        .unwrap();
    let off = nemesis_config::BoardFlagConfig::default();
    assert!(!super::auto_dispatch_with_config(
        Some(&off),
        &store,
        None,
        &issue2,
        &actor,
    ));
    let comments2 = store.list_comments(issue2.id).unwrap();
    assert!(
        !comments2.iter().any(|c| c.content.contains("自动派发失败")),
        "gate off → no trace comment expected"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Swarm M1：issue.plan 两段式拆解 + 依赖补派（cluster 编译时；无真实集群的
// 路径全部走诚实降级断言——自动派发的正路径由真机矩阵 G1/G2 覆盖）
// ---------------------------------------------------------------------------

/// 取 ctx 里 board store 的 Arc。
#[cfg(feature = "cluster")]
fn store_of(ctx: &RequestContext) -> Arc<BoardStore> {
    ctx.state.board.as_ref().unwrap().store().clone()
}

/// 把 plan 预览直接种进缓存（绕过 LLM 一段；LLM 路径由 nemesis-board
/// planner 测试 + nemesis-agent spawn_detached 测试覆盖）。
#[cfg(feature = "cluster")]
fn seed_plan(plan_id: &str, issue_id: i64, subs: Vec<nemesis_board::PlannedSubIssue>) {
    super::PLAN_CACHE.lock().insert(
        plan_id.to_string(),
        super::PlanPreview {
            issue_id,
            subs,
            downgrades: Vec::new(),
            created_at: Instant::now(),
        },
    );
}

#[cfg(feature = "cluster")]
fn sub(title: &str, deps: Vec<usize>) -> nemesis_board::PlannedSubIssue {
    nemesis_board::PlannedSubIssue {
        title: title.to_string(),
        description: format!("{title} 的说明"),
        required_role: String::new(),
        required_tags: Vec::new(),
        required_profession: String::new(),
        required_backend: String::new(),
        required_capabilities: vec![],
        acceptance_criteria: format!("- [ ] {title} 验收"),
        depends_on: deps,
    }
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_plan_missing_id_rejected() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-miss-id");
    let ctx = make_ctx_with_board(&dir);
    let err = dispatch(&ctx, "issue.plan", serde_json::json!({}))
        .await
        .expect_err("missing id must reject");
    assert!(err.contains("missing field: id"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_plan_planning_requires_agent() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-no-agent");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "父任务" }),
    )
    .await
    .unwrap();
    let id = out.unwrap()["issue"]["id"].as_i64().unwrap();

    // agent_loop 未注入（AppState 测试臂恒 None）→ 诚实报错，不静默。
    let err = dispatch(&ctx, "issue.plan", serde_json::json!({ "id": id }))
        .await
        .expect_err("no agent loop must reject");
    assert!(err.contains("agent 未运行"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_plan_confirm_unknown_plan_id_rejected() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-unknown-id");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "父任务" }),
    )
    .await
    .unwrap();
    let id = out.unwrap()["issue"]["id"].as_i64().unwrap();

    let err = dispatch(
        &ctx,
        "issue.plan",
        serde_json::json!({ "id": id, "plan_id": "plan-nope", "confirm": true }),
    )
    .await
    .expect_err("unknown plan_id must reject");
    assert!(err.contains("不存在或已被消费"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_plan_confirm_wrong_issue_preserves_cache_and_consumes_once() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-mismatch");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "父任务 A".into(),
            ..Default::default()
        })
        .unwrap();
    let b = store
        .create_issue(nemesis_board::NewIssue {
            title: "父任务 B".into(),
            ..Default::default()
        })
        .unwrap();
    seed_plan("plan-t3", a.id, vec![sub("子任务", vec![])]);

    // 贴错 issue → 拒绝且缓存保留（不误伤在途预览）。
    let err = dispatch(
        &ctx,
        "issue.plan",
        serde_json::json!({ "id": b.id, "plan_id": "plan-t3", "confirm": true }),
    )
    .await
    .expect_err("mismatched issue must reject");
    assert!(err.contains("不匹配"), "{err}");
    assert!(super::PLAN_CACHE.lock().contains_key("plan-t3"));

    // 正主确认 → 成功；且缓存一次性消费（重复确认拒绝）。
    let out = dispatch(
        &ctx,
        "issue.plan",
        serde_json::json!({ "id": a.id, "plan_id": "plan-t3", "confirm": true }),
    )
    .await
    .unwrap();
    let out = out.unwrap();
    assert_eq!(out["created"].as_array().unwrap().len(), 1);
    let err = dispatch(
        &ctx,
        "issue.plan",
        serde_json::json!({ "id": a.id, "plan_id": "plan-t3", "confirm": true }),
    )
    .await
    .expect_err("double confirm must reject");
    assert!(err.contains("不存在或已被消费"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_plan_confirm_creates_children_with_deps_origin_and_honest_degrade() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-confirm");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let out = dispatch(
        &ctx,
        "issue.create",
        serde_json::json!({ "title": "重构存储层", "description": "拆成三步" }),
    )
    .await
    .unwrap();
    let parent_id = out.unwrap()["issue"]["id"].as_i64().unwrap();

    let mut s1 = sub("步骤一：抽接口", vec![]);
    s1.required_role = "worker".into();
    s1.required_tags = vec!["rust".into()];
    let s2 = sub("步骤二：换实现", vec![0]);
    let s3 = sub("步骤三：回归验证", vec![0, 1]);
    seed_plan("plan-t4", parent_id, vec![s1, s2, s3]);

    let out = dispatch(
        &ctx,
        "issue.plan",
        serde_json::json!({ "id": parent_id, "plan_id": "plan-t4", "confirm": true }),
    )
    .await
    .unwrap()
    .unwrap();
    let created: Vec<i64> = out["created"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(created.len(), 3);
    assert_eq!(out["dispatched"], 0, "无集群 → 派发诚实降级为失败评论");

    // 落库语义：parent / origin=planner / required_* / 批内序号→id 依赖边。
    let [ia, ib, ic] = [created[0], created[1], created[2]];
    let child_a = store.get_issue(ia).unwrap();
    assert_eq!(child_a.parent_issue_id, Some(parent_id));
    assert_eq!(child_a.required_role.as_deref(), Some("worker"));
    assert_eq!(child_a.required_tags, vec!["rust".to_string()]);
    let origin = child_a.origin.as_ref().unwrap();
    assert_eq!(origin.origin_type, "planner");
    assert_eq!(origin.origin_id, format!("NB-{parent_id}"));
    assert_eq!(store.dependencies_of(ib).unwrap(), vec![ia]);
    assert_eq!(store.dependencies_of(ic).unwrap(), vec![ia, ib]);
    for cid in &created {
        assert_eq!(store.get_issue(*cid).unwrap().status, IssueStatus::Backlog);
    }
    // 无集群 → 首张（无依赖、走到派发核心）留诚实降级评论；后两张依赖
    // 未满足（首张没派出去）→ 走依赖闸暂缓，进响应 deferred 数组（标准
    // 流程，不刷评论——补派触发器会在依赖 done 后重试）。
    let deferred: Vec<i64> = out["deferred"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(deferred, vec![ib, ic]);
    let comments_a = store.list_comments(ia).unwrap();
    assert!(
        comments_a
            .iter()
            .any(|c| c.content.contains("自动派发失败")),
        "first child should carry honest degrade comment"
    );
    assert!(store.list_comments(ib).unwrap().is_empty());
    // 无派发 → 父单保持 backlog。
    assert_eq!(
        store.get_issue(parent_id).unwrap().status,
        IssueStatus::Backlog
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn dispatch_subissue_auto_gates_dependency_status_and_active_dispatch() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("auto-gates");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "A".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let b = store
        .create_issue(nemesis_board::NewIssue {
            title: "B".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    store.set_issue_dependencies(b.id, &[a.id]).unwrap();

    // 依赖未满足 → Ok(None)（暂缓，不报错）。
    assert_eq!(
        super::dispatch_subissue_auto(&store, None, b.id, &actor, true).unwrap(),
        None
    );
    // 非待派状态（在途）→ Ok(None)。
    store
        .transition_issue(a.id, IssueStatus::InProgress, &actor)
        .unwrap();
    assert_eq!(
        super::dispatch_subissue_auto(&store, None, a.id, &actor, true).unwrap(),
        None
    );
    // 依赖 done 后放行 → 无集群时走到派发核心，诚实报集群缺失（证明
    // 已越过依赖闸）。
    store
        .transition_issue(a.id, IssueStatus::Done, &actor)
        .unwrap();
    let err = super::dispatch_subissue_auto(&store, None, b.id, &actor, true)
        .expect_err("past dep gate + no cluster must reach dispatch core error");
    assert!(err.contains("集群未运行"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn sync_parent_status_all_done_cancel_gap_and_terminal_noop() {
    let dir = unique_dir("parent-sync");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");

    // 场景 1：backlog 父单 + 全子单 done → 垫 in_progress 后进 in_review。
    let p1 = store
        .create_issue(nemesis_board::NewIssue {
            title: "父一".into(),
            ..Default::default()
        })
        .unwrap();
    let c1 = store
        .create_issue(nemesis_board::NewIssue {
            title: "子一".into(),
            parent_issue_id: Some(p1.id),
            ..Default::default()
        })
        .unwrap();
    let c2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "子二".into(),
            parent_issue_id: Some(p1.id),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(c1.id, IssueStatus::Done, &actor)
        .unwrap();
    store
        .transition_issue(c2.id, IssueStatus::Done, &actor)
        .unwrap();
    super::sync_parent_status(&store, p1.id, &actor).unwrap();
    assert_eq!(
        store.get_issue(p1.id).unwrap().status,
        IssueStatus::InReview
    );

    // 场景 2：有子单 cancelled → 收口 in_review + 系统评论标注缺口。
    let p2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "父二".into(),
            ..Default::default()
        })
        .unwrap();
    let d1 = store
        .create_issue(nemesis_board::NewIssue {
            title: "子甲".into(),
            parent_issue_id: Some(p2.id),
            ..Default::default()
        })
        .unwrap();
    let d2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "子乙".into(),
            parent_issue_id: Some(p2.id),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(d1.id, IssueStatus::Done, &actor)
        .unwrap();
    store
        .transition_issue(d2.id, IssueStatus::Cancelled, &actor)
        .unwrap();
    super::sync_parent_status(&store, p2.id, &actor).unwrap();
    assert_eq!(
        store.get_issue(p2.id).unwrap().status,
        IssueStatus::InReview
    );
    let comments = store.list_comments(p2.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("人工裁决") && c.content.contains("cancelled")),
        "gap comment expected, got {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );

    // 场景 3：终态父单 no-op。
    store
        .transition_issue(p2.id, IssueStatus::Cancelled, &actor)
        .unwrap();
    super::sync_parent_status(&store, p2.id, &actor).unwrap();
    assert_eq!(
        store.get_issue(p2.id).unwrap().status,
        IssueStatus::Cancelled
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn on_issue_settled_syncs_parent_and_attempts_redispatch() {
    let dir = unique_dir("settled");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "A".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let b = store
        .create_issue(nemesis_board::NewIssue {
            title: "B".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    store.set_issue_dependencies(b.id, &[a.id]).unwrap();

    // A 落定 done：父单联动（B 未 done → 不收口）+ B 补派尝试（依赖闸
    // 已放行，无集群 → 派发核心报错，仅 warn 留痕——集群缺失是系统性
    // 状态，不逐单刷评论）。B 保持 backlog 等真机路径。
    store
        .transition_issue(a.id, IssueStatus::Done, &actor)
        .unwrap();
    super::on_issue_settled(&store, None, a.id, &actor);
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::Backlog
    );
    let b_issue = store.get_issue(b.id).unwrap();
    assert_eq!(b_issue.status, IssueStatus::Backlog);
    assert!(store.list_comments(b.id).unwrap().is_empty());

    // B 也落定 done → 父单收口 in_review。
    store
        .transition_issue(b.id, IssueStatus::Done, &actor)
        .unwrap();
    super::on_issue_settled(&store, None, b.id, &actor);
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------------------------
// Swarm M3 批次 E：讨论频道（channel.list / channel.messages / channel.post）
// -------------------------------------------------------------------------

/// 假发言桥：记录调用并回固定首响（不写 store——web 层只负责桥接）。
struct FakeIngress(std::sync::Mutex<Vec<(String, String, i64, String)>>);

impl nemesis_board::service::DiscussionIngress for FakeIngress {
    fn post(
        &self,
        sender: &Actor,
        thread_kind: &str,
        thread_id: i64,
        _client_msg_id: &str,
        content: &str,
        _reply_to: Option<i64>,
        _kind_tag: &str,
    ) -> Result<serde_json::Value, String> {
        self.0.lock().unwrap().push((
            format!("{}/{}", sender.kind, sender.id),
            thread_kind.to_string(),
            thread_id,
            content.to_string(),
        ));
        Ok(serde_json::json!({ "message_id": 42, "seq": 7 }))
    }
}

#[tokio::test]
async fn test_channel_list_and_messages_pagination() {
    let dir = unique_dir("channel-list");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();
    let ctx = make_ctx_with_service(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
    );

    let out = dispatch(&ctx, "channel.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let channels = out["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 4, "default #dev/#qa/#general/#team seeded");
    let cid = channels[0]["id"].as_i64().unwrap();

    // 直接经 store 造 3 条消息，验证 after_id 游标与 limit。
    for i in 0..3 {
        store
            .append_channel_message(nemesis_board::NewChannelMessage {
                channel_id: cid,
                sender: Actor::agent("node-b"),
                content: format!("msg-{i}"),
                parent_id: None,
                mtype: "text".to_string(),
            })
            .unwrap();
    }
    let out = dispatch(
        &ctx,
        "channel.messages",
        serde_json::json!({ "channel_id": cid }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["messages"].as_array().unwrap().len(), 3);

    let out = dispatch(
        &ctx,
        "channel.messages",
        serde_json::json!({ "channel_id": cid, "after_id": 2 }),
    )
    .await
    .unwrap()
    .unwrap();
    let msgs = out["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], 3);

    let out = dispatch(
        &ctx,
        "channel.messages",
        serde_json::json!({ "channel_id": cid, "limit": 2 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["messages"].as_array().unwrap().len(), 2);

    // 缺 channel_id → 报错。
    assert!(
        dispatch(&ctx, "channel.messages", serde_json::json!({}))
            .await
            .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_channel_post_routes_through_ingress() {
    let dir = unique_dir("channel-post");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();
    let cid = store.list_channels().unwrap()[0].id;

    // 桥未装配 → 诚实拒绝。
    let ctx = make_ctx_with_service(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
    );
    let err = dispatch(
        &ctx,
        "channel.post",
        serde_json::json!({ "channel_id": cid, "content": "hi" }),
    )
    .await
    .expect_err("no ingress → honest error");
    assert!(err.contains("讨论总线未装配"), "got: {err}");

    // 注入假桥 → 发言经桥转发（sender=dashboard admin，内容原样）。
    let fake = Arc::new(FakeIngress(std::sync::Mutex::new(Vec::new())));
    let ctx = make_ctx_with_service(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator)
            .with_discussion(fake.clone()),
    );
    let out = dispatch(
        &ctx,
        "channel.post",
        serde_json::json!({ "channel_id": cid, "content": "帮我看看这个报错" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["posted"]["message_id"], 42);
    {
        let calls = fake.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "admin/dashboard");
        assert_eq!(calls[0].2, cid);
        assert_eq!(calls[0].3, "帮我看看这个报错");
    }

    // 空 content / 缺参 → 报错。
    assert!(
        dispatch(
            &ctx,
            "channel.post",
            serde_json::json!({ "channel_id": cid, "content": "  " })
        )
        .await
        .is_err()
    );
    assert!(
        dispatch(&ctx, "channel.post", serde_json::json!({ "content": "x" }))
            .await
            .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 全自动流转 P1（A4）：board.config.get / board.config.set 配置 TAB 读写。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_board_config_get_set_roundtrip() {
    let dir = unique_dir("config-roundtrip");
    let ctx = make_ctx_with_board(&dir);

    // 初始 get：board 段全默认（新键 auto_close_parent / unlimited_mode 均 false）。
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["auto_close_parent"], false);
    assert_eq!(out["unlimited_mode"], false);

    // set → updated:true；get 回读；盘上真实落盘。
    let out = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "unlimited_mode", "value": true }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["unlimited_mode"], true);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["board"]["unlimited_mode"], true);

    // 嵌套键（plan.auto_confirm）与数字键同路。
    dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "plan.auto_confirm", "value": true }),
    )
    .await
    .unwrap()
    .unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["board"]["plan"]["auto_confirm"], true);

    let _ = std::fs::remove_dir_all(&dir);
}

// P5/F1：conflict_auto_resolve 在 config.set 白名单内（缺省 false=human 档；
// 开 = AI 硬解漏斗）。这是 Dashboard 配置页切换冲突档位的唯一写入口。
#[tokio::test]
async fn test_board_config_set_conflict_auto_resolve_roundtrip() {
    let dir = unique_dir("config-conflict-auto-resolve");
    let ctx = make_ctx_with_board(&dir);

    // 初始 get：缺省 false（human 档）。
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["conflict_auto_resolve"], false);

    // set true → get 回读 + 盘上真实落盘。
    let out = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "conflict_auto_resolve", "value": true }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["conflict_auto_resolve"], true);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["board"]["conflict_auto_resolve"], true);

    // 非布尔 loud 拒绝。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "conflict_auto_resolve", "value": "yes" }),
    )
    .await
    .expect_err("非布尔必须拒绝");
    assert!(err.contains("布尔"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

// 连续 UNSURE 停车阈值（2026-10-15 用户：设置页缺旋钮）：白名单 + 回读 +
// 盘上落盘 roundtrip；0=不限合法。
#[tokio::test]
async fn test_board_config_set_max_unsure_streak_roundtrip() {
    let dir = unique_dir("config-max-unsure-streak");
    let ctx = make_ctx_with_board(&dir);

    // 初始 get：缺省 2（BoardReviewConfig::default 同源）。
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["review"]["max_unsure_streak"], 2);

    // set 11 → get 回读 + 盘上真实落盘（用户实测改 11 的场景）。
    let out = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.max_unsure_streak", "value": 11 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["review"]["max_unsure_streak"], 11);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["board"]["review"]["max_unsure_streak"], 11);

    // 0=不限合法。
    let out = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.max_unsure_streak", "value": 0 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);

    // 非整数 loud 拒绝。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.max_unsure_streak", "value": "many" }),
    )
    .await
    .expect_err("非整数必须拒绝");
    assert!(err.contains("非负整数"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_board_config_set_unknown_key_loud_rejects() {
    let dir = unique_dir("config-unknown-key");
    let ctx = make_ctx_with_board(&dir);

    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "evil_key", "value": true }),
    )
    .await
    .expect_err("whitelist外的键必须 loud 拒绝");
    assert!(err.contains("未知或不允许"), "got: {err}");
    assert!(
        err.contains("unlimited_mode"),
        "错误信息应列出允许键: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_board_config_set_wrong_type_rejects() {
    let dir = unique_dir("config-wrong-type");
    let ctx = make_ctx_with_board(&dir);

    // bool 键收到字符串 → 拒绝且盘上未半写。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "auto_accept", "value": "on" }),
    )
    .await
    .expect_err("wrong typed value must be rejected");
    assert!(err.contains("布尔"), "got: {err}");
    if let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) {
        let raw: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(raw["board"]["auto_accept"], false, "拒绝后不得半写");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// B1（职能→评审预设查表）：review.professions.<slug>.<field> 白名单键。
// 写入 → get 回读 + 盘上真实落盘；非法 slug/未知维度 loud 拒绝。
#[tokio::test]
async fn test_board_config_set_profession_preset_roundtrip() {
    let dir = unique_dir("config-profession-preset");
    let ctx = make_ctx_with_board(&dir);

    // 写两个 slug × 不同维度（部分维度缺省=Option None 语义）。
    for (key, value) in [
        ("review.professions.Product.max_turns", serde_json::json!(0)),
        ("review.professions.Product.checkers", serde_json::json!(2)),
        (
            "review.professions.Product.selfcheck",
            serde_json::json!(true),
        ),
        ("review.professions.dev:cuda.checkers", serde_json::json!(3)),
    ] {
        let out = dispatch(
            &ctx,
            "config.set",
            serde_json::json!({ "key": key, "value": value }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(out["updated"], true, "{key}");
    }

    // get 回读：slug 大小写归一（Product → product），嵌套结构完整。
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let product = &out["review"]["professions"]["product"];
    assert_eq!(product["max_turns"], 0);
    assert_eq!(product["checkers"], 2);
    assert_eq!(product["selfcheck"], true);
    assert_eq!(out["review"]["professions"]["dev:cuda"]["checkers"], 3);

    // 盘上真实落盘。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        raw["board"]["review"]["professions"]["product"]["selfcheck"],
        true
    );

    // 非法 slug loud 拒绝（含空格/大写归一后仍非法的形态）。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.professions.BAD SLUG.checkers", "value": 1 }),
    )
    .await
    .expect_err("非法 slug 必须拒绝");
    assert!(err.contains("非法"), "got: {err}");

    // 未知维度 loud 拒绝。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.professions.dev.tier", "value": "big" }),
    )
    .await
    .expect_err("未知维度必须拒绝");
    assert!(err.contains("未知职能预设维度"), "got: {err}");

    // checkers 越界拒绝（消费侧 1..=5 收敛的前置镜像）。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.professions.dev.checkers", "value": 9 }),
    )
    .await
    .expect_err("checkers 越界必须拒绝");
    assert!(err.contains("1..=5"), "got: {err}");

    // null = 清除维度覆盖；三维度全空条目顺手删除（幂等，重复 null 不报错）。
    for key in [
        "review.professions.product.max_turns",
        "review.professions.product.checkers",
        "review.professions.product.selfcheck",
        "review.professions.dev:cuda.checkers",
        "review.professions.dev:cuda.checkers",
    ] {
        dispatch(
            &ctx,
            "config.set",
            serde_json::json!({ "key": key, "value": null }),
        )
        .await
        .unwrap()
        .unwrap();
    }
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert!(
        out["review"]["professions"].as_object().unwrap().is_empty(),
        "null 清除后 professions map 应为空: {:?}",
        out["review"]["professions"]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 全自动流转 P3：D2 autopilot auto_plan / F1 project auto_start / F2 项目
// 状态联动
// ---------------------------------------------------------------------------

/// 建一条 target 空（不预指派）+ 可调 auto_plan 的规则（经 store 落库——
/// fire_autopilot 尾部 mark_autopilot_run 要求规则真实存在；顺带覆盖
/// auto_plan 的 DB 写读回路）。
#[cfg(feature = "cluster")]
fn p3_ap(store: &BoardStore, name: &str, auto_plan: bool) -> nemesis_board::Autopilot {
    store
        .create_autopilot(&nemesis_board::NewAutopilot {
            name: name.to_string(),
            cron: "0 9 * * *".to_string(),
            title: "周期任务 {date}".to_string(),
            description: "P3 测试".to_string(),
            priority: priority::MEDIUM,
            project_id: None,
            target: String::new(),
            enabled: true,
            auto_plan,
            acceptance_criteria: None,
        })
        .expect("create autopilot")
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_fire_autopilot_auto_plan_false_unchanged() {
    // auto_plan=false（存量默认）→ 行为与 P3 之前一致：只建单，auto_plan=null。
    let dir = unique_dir("ap-auto-plan-false");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let actor = nemesis_board::Actor::admin("admin");
    let out = fire_autopilot(&store, None, &p3_ap(&store, "常规", false), &actor, None).unwrap();
    assert_eq!(out["ran"], true);
    assert!(out["auto_plan"].is_null(), "auto_plan=false → null: {out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_fire_autopilot_passes_acceptance_criteria_to_issue() {
    // F-U5-1：规则带验收标准 → 触发建单透传给 issue（定时任务全自动验收
    // 的前提）；规则不填 → issue 验收标准为空（保守转人工语义不变）。
    let dir = unique_dir("ap-acceptance-passthrough");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let actor = nemesis_board::Actor::admin("admin");

    let n = nemesis_board::NewAutopilot {
        name: "带标准".to_string(),
        cron: "0 9 * * *".to_string(),
        title: "周期任务 {date}".to_string(),
        description: "P3 测试".to_string(),
        priority: nemesis_board::models::priority::MEDIUM,
        project_id: None,
        target: String::new(),
        enabled: true,
        auto_plan: false,
        acceptance_criteria: Some("报告含时间与磁盘空间两项".to_string()),
    };
    let ap = store.create_autopilot(&n).unwrap();
    let out = fire_autopilot(&store, None, &ap, &actor, None).unwrap();
    let issue = store.get_issue(out["issue_id"].as_i64().unwrap()).unwrap();
    assert_eq!(
        issue.acceptance_criteria.as_deref(),
        Some("报告含时间与磁盘空间两项"),
        "验收标准应透传到 issue: {:?}",
        issue.acceptance_criteria
    );

    let ap_empty = p3_ap(&store, "不填", false);
    let out2 = fire_autopilot(&store, None, &ap_empty, &actor, None).unwrap();
    let issue2 = store.get_issue(out2["issue_id"].as_i64().unwrap()).unwrap();
    // 2026-10-17 契约更新（NB-125 根修）：规则不填 → issue 萭 store 自动
    // 生成 AC（任务形态）——此前「为空保守转人工」语义被自动生成取代，
    // 定时建单同样不再裸奔进评审。
    assert!(
        issue2
            .acceptance_criteria
            .as_deref()
            .map(|ac| ac.starts_with("（系统自动生成）") && ac.contains("目标已达成并验证"))
            .unwrap_or(false),
        "规则不填 → 自动生成 AC: {:?}",
        issue2.acceptance_criteria
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_fire_autopilot_auto_plan_true_without_context_skips_honestly() {
    // CLI 手跑（auto_plan=None）+ auto_plan=true → skipped 说明，不报错不炸。
    let dir = unique_dir("ap-auto-plan-cli");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let actor = nemesis_board::Actor::admin("admin");
    let out = fire_autopilot(&store, None, &p3_ap(&store, "CLI 触发", true), &actor, None).unwrap();
    assert_eq!(out["auto_plan"]["status"], "skipped");
    assert!(
        out["auto_plan"]["reason"]
            .as_str()
            .unwrap()
            .contains("无 moderator/事件上下文"),
        "reason 应说明入口缺上下文: {out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_fire_autopilot_auto_plan_true_empty_slot_degrades_with_comment() {
    // 槽空（moderator 未装配）→ WARN + 系统评论诚实降级，cron 回调不炸。
    let dir = unique_dir("ap-auto-plan-empty-slot");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let actor = nemesis_board::Actor::admin("admin");
    let ap_ctx = AutoPlanContext {
        moderator_slot: Arc::new(std::sync::OnceLock::new()), // 永不填充
        home: dir.clone(),
        hub: None,
        cluster: None,
    };
    let out = fire_autopilot(
        &store,
        None,
        &p3_ap(&store, "槽空", true),
        &actor,
        Some(&ap_ctx),
    )
    .unwrap();
    assert_eq!(out["auto_plan"]["status"], "skipped");
    assert_eq!(out["auto_plan"]["reason"], "moderator loop 未就绪");
    // 系统评论落痕（下轮触发自动恢复的可观测性）。
    let issue_id = out["issue_id"].as_i64().unwrap();
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("moderator agent 未就绪")),
        "应有降级系统评论: {comments:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_project_create_auto_start_builds_parent_issue() {
    // F1：auto_start=true → 建父单（title=项目名 / project_id 绑定 /
    // origin=project / acceptance_criteria 透传）；agent 未运行 → 诚实降级
    // 评论一次（幂等：不重复发车）。
    let dir = unique_dir("project-auto-start");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let out = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({
            "name": "自动启动项目",
            "description": "建项目即启动",
            "acceptance_criteria": "1. 父单自动建\n2. 拆解可发车",
            "auto_start": true,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let pid = out["project"]["id"].as_i64().unwrap();
    let started = out["auto_start"]["issue_number"]
        .as_str()
        .expect("auto_start 应返回父单单号");
    let issue = store.get_issue_by_number(started).expect("父单应已落库");
    assert_eq!(issue.title, "自动启动项目");
    assert_eq!(issue.project_id, Some(pid));
    assert_eq!(
        issue.acceptance_criteria.as_deref(),
        Some("1. 父单自动建\n2. 拆解可发车")
    );
    assert_eq!(
        issue.origin.as_ref().map(|o| o.origin_type.as_str()),
        Some("project")
    );
    assert_eq!(
        issue.origin.as_ref().map(|o| o.origin_id.as_str()),
        Some(pid.to_string().as_str())
    );
    // agent 未运行（测试 ctx.agent_loop=None）→ 降级评论恰一条。
    let comments = store.list_comments(issue.id).unwrap();
    let degrades = comments
        .iter()
        .filter(|c| c.content.contains("未自动拆解"))
        .count();
    assert_eq!(degrades, 1, "诚实降级评论应恰好一条: {comments:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_project_create_auto_start_false_keeps_legacy_behavior() {
    // auto_start 缺省 false（保守默认）→ 不建父单，响应无 auto_start 字段。
    let dir = unique_dir("project-no-auto-start");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "普通项目", "description": "x" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(out.get("auto_start").is_none(), "存量行为不变: {out}");
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let issues = store
        .list_issues(&nemesis_board::IssueFilter::default())
        .unwrap();
    assert!(issues.is_empty(), "不应有父单: {issues:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_link_project_on_dispatch_transitions_and_guards() {
    // F2：Active + project_id 非空 → in_progress；project_id 空 / 非 Active
    // 状态不联动（回归保护）。
    let dir = unique_dir("link-project-dispatch");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let pid = store
        .create_project("联动项目", "", None, "", "", None)
        .unwrap()
        .id;
    // ① project_id=None → 不联动。
    link_project_on_dispatch(&store, None);
    assert_eq!(store.get_project(pid).unwrap().status, "active");
    // ② Active + 有 project_id → in_progress。
    link_project_on_dispatch(&store, Some(pid));
    assert_eq!(store.get_project(pid).unwrap().status, "in_progress");
    // ③ 非 Active（已 in_progress）→ 不动（不回退不重复写）。
    link_project_on_dispatch(&store, Some(pid));
    assert_eq!(store.get_project(pid).unwrap().status, "in_progress");
    // ④ archived → 不联动。
    store
        .update_project(
            pid,
            &nemesis_board::ProjectPatch {
                status: Some("archived".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    link_project_on_dispatch(&store, Some(pid));
    assert_eq!(store.get_project(pid).unwrap().status, "archived");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_confirm_plan_subs_inherit_parent_project_id() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    // F2 回归（T32 实证缺口）：confirm_plan 落库的子单必须继承父单
    // project_id——派发联动 link_project_on_dispatch 读的是子单自己的
    // project_id，缺继承则项目永远停在 active。
    let dir = unique_dir("confirm-plan-inherit-project");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let pid = store
        .create_project("继承项目", "", None, "", "", None)
        .unwrap()
        .id;
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父单".to_string(),
            project_id: Some(pid),
            creator: nemesis_board::Actor::system("board"),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();
    confirm_plan(
        &store,
        None, // 集群未装配 → 子单派发诚实降级（不撞本断言关注点）
        &parent,
        vec![nemesis_board::PlannedSubIssue {
            title: "子单".to_string(),
            description: String::new(),
            required_role: String::new(),
            required_tags: Vec::new(),
            required_profession: String::new(),
            required_backend: String::new(),
            required_capabilities: vec![],
            acceptance_criteria: String::new(),
            depends_on: Vec::new(),
        }],
        &nemesis_board::Actor::system("board"),
        &[],
    )
    .unwrap();
    // 按 project 过滤 + 标题查询定位子单（同时验证项目过滤视图含子单）。
    let subs = store
        .list_issues(&nemesis_board::IssueFilter {
            project_id: Some(pid),
            query: Some("子单".to_string()),
            ..nemesis_board::IssueFilter::default()
        })
        .unwrap();
    assert_eq!(subs.len(), 1, "项目视图应恰好含这个子单: {subs:?}");
    assert_eq!(
        subs[0].project_id,
        Some(pid),
        "子单必须继承父单 project_id（F2 联动数据源）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_issue_plan_via_handler_one_shot_planning() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    // pub 化回归保护：WSAPI issue.plan 一段入口行为不变（async 规划中，
    // agent 未运行时诚实报错——报错文案带「agent 未运行」）。
    let dir = unique_dir("issue-plan-regression");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "待拆解".to_string(),
            ..Default::default()
        })
        .unwrap();
    let err = dispatch(&ctx, "issue.plan", serde_json::json!({ "id": issue.id }))
        .await
        .expect_err("agent 未运行必须诚实报错");
    assert!(err.contains("agent 未运行"), "got: {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 全自动流转 P4：F3 项目收口触发面（eligible 矩阵 + notify 守卫）+ config
// review/budget 新键回读
// ---------------------------------------------------------------------------

/// F3 eligible 夹具：项目 + 若干顶层父单（前 done_parents 个落 done）。
#[cfg(feature = "cluster")]
fn f3_project_with_parents(
    store: &BoardStore,
    name: &str,
    parents: usize,
    done_parents: usize,
) -> i64 {
    use nemesis_board::models::IssueStatus;
    let pid = store
        .create_project(name, "", None, "", "", None)
        .unwrap()
        .id;
    let creator = nemesis_board::Actor::agent("node-a");
    for i in 0..parents {
        let issue = store
            .create_issue(nemesis_board::NewIssue {
                title: format!("{name}父{i}"),
                creator: creator.clone(),
                project_id: Some(pid),
                ..nemesis_board::NewIssue::default()
            })
            .unwrap();
        if i < done_parents {
            store
                .transition_issue(issue.id, IssueStatus::InProgress, &creator)
                .unwrap();
            store
                .transition_issue(issue.id, IssueStatus::Done, &creator)
                .unwrap();
        }
    }
    pid
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_project_completion_eligible_matrix() {
    let dir = unique_dir("f3-eligible");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);

    // 不存在的项目 → false。
    assert!(!project_completion_eligible(&store, 999_999));

    // 空项目（无顶层父单）→ false。
    let empty = store
        .create_project("空项目", "", None, "", "", None)
        .unwrap()
        .id;
    assert!(!project_completion_eligible(&store, empty));

    // 全部顶层父单 done → true（F2 首派联动后项目已在 in_progress 主链）。
    let all_done = f3_project_with_parents(&store, "全done项目", 2, 2);
    assert!(project_completion_eligible(&store, all_done));

    // 存在未 done 顶层父单 → false。
    let partial = f3_project_with_parents(&store, "半done项目", 2, 1);
    assert!(!project_completion_eligible(&store, partial));

    // 子单 done 但父单未 done → false（只有顶层父单是触发面）。
    let pid = store
        .create_project("子done项目", "", None, "", "", None)
        .unwrap()
        .id;
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "未done父单".to_string(),
            creator: creator.clone(),
            project_id: Some(pid),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();
    let child = store
        .create_issue(nemesis_board::NewIssue {
            title: "done子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            project_id: Some(pid),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();
    store
        .transition_issue(child.id, nemesis_board::models::IssueStatus::Done, &creator)
        .unwrap();
    assert!(!project_completion_eligible(&store, pid));

    // completed 项目 → false（已收口不重触发）。
    store
        .update_project(
            all_done,
            &nemesis_board::ProjectPatch {
                status: Some("in_progress".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .update_project(
            all_done,
            &nemesis_board::ProjectPatch {
                status: Some("completed".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!project_completion_eligible(&store, all_done));

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_notify_project_review_guards_and_hook_fire() {
    let dir = unique_dir("f3-notify");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    // 进程级 OnceLock：装配权可能被 agt_hook_setters 用例抢先（同一共享
    // 录制器 AGT_PROJECT_REVIEW_FIRED，赢家装配）；set 失败 = 已注册同源
    // 闭包，直接用。先清空录制器保证断言与本用例自身触发一一对应。
    let _ = set_project_review_hook(std::sync::Arc::new(|pid| {
        AGT_PROJECT_REVIEW_FIRED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(pid);
    }));
    AGT_PROJECT_REVIEW_FIRED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    let fired = &AGT_PROJECT_REVIEW_FIRED;

    // ① 子单 done：非触发面，早退不 fire。
    let pid = f3_project_with_parents(&store, "notify全done", 1, 1);
    let top = store
        .list_issues(&nemesis_board::IssueFilter {
            project_id: Some(pid),
            ..nemesis_board::IssueFilter::default()
        })
        .unwrap()
        .into_iter()
        .find(|i| i.parent_issue_id.is_none())
        .unwrap();
    let sub = store
        .create_issue(nemesis_board::NewIssue {
            title: "notify子单".to_string(),
            creator: nemesis_board::Actor::agent("node-a"),
            parent_issue_id: Some(top.id),
            project_id: Some(pid),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();
    notify_project_review_on_parent_done(&store, sub.id);
    assert!(fired.lock().unwrap().is_empty(), "子单不得触发项目收口");

    // ② 合格项目 + 顶层父单 done（已是 done，eligible=true）→ fire 一次。
    notify_project_review_on_parent_done(&store, top.id);
    assert_eq!(
        fired.lock().unwrap().as_slice(),
        &[pid],
        "合格项目的顶层父单必须触发一次收口 hook"
    );

    // ③ 不合格项目（父单未全 done）→ 不 fire。
    let partial = f3_project_with_parents(&store, "notify半done", 2, 1);
    let partial_top = store
        .list_issues(&nemesis_board::IssueFilter {
            project_id: Some(partial),
            ..nemesis_board::IssueFilter::default()
        })
        .unwrap()
        .into_iter()
        .find(|i| i.parent_issue_id.is_none())
        .unwrap();
    notify_project_review_on_parent_done(&store, partial_top.id);
    assert_eq!(fired.lock().unwrap().len(), 1, "不合格项目不得追加 fire");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_board_config_review_budget_keys_roundtrip() {
    let dir = unique_dir("config-review-budget");
    let ctx = make_ctx_with_board(&dir);

    // 初始默认值回读（P4 新键全部 serde default）。
    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    // M6-3（2026-10-10）：评审取证轮数默认 0=不限（C1 预算耗尽根修；
    // doom-loop/estop 守卫仍在，0 语义安全）。
    assert_eq!(out["review"]["max_turns"], 0);
    assert_eq!(out["review"]["selfcheck"], false);
    assert_eq!(out["review"]["auto_close_project"], false);
    assert_eq!(out["budget"]["max_subissues_per_parent"], 20);
    assert_eq!(out["budget"]["max_total_redispatch"], 0);
    assert_eq!(out["budget"]["wall_clock_budget_secs"], 0);
    assert_eq!(out["review"]["checkers"], 1);
    assert_eq!(out["budget"]["max_tokens_per_parent"], 0);
    // F2 送达失败重试熔断默认开（5 次，自动循环默认带护栏）。
    assert_eq!(out["budget"]["max_send_retries"], 5);

    // 九键 set → 盘上真实落盘回读。
    for (key, value) in [
        ("review.max_turns", serde_json::json!(3)),
        ("review.selfcheck", serde_json::json!(true)),
        ("review.auto_close_project", serde_json::json!(true)),
        ("budget.max_subissues_per_parent", serde_json::json!(10)),
        ("budget.max_total_redispatch", serde_json::json!(5)),
        ("budget.wall_clock_budget_secs", serde_json::json!(3600)),
        ("review.checkers", serde_json::json!(3)),
        ("budget.max_tokens_per_parent", serde_json::json!(100000)),
        ("budget.max_send_retries", serde_json::json!(7)),
    ] {
        let out = dispatch(
            &ctx,
            "config.set",
            serde_json::json!({ "key": key, "value": value }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(out["updated"], true, "set {key} 应成功");
    }
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["board"]["review"]["max_turns"], 3);
    assert_eq!(raw["board"]["review"]["selfcheck"], true);
    assert_eq!(raw["board"]["review"]["auto_close_project"], true);
    assert_eq!(raw["board"]["budget"]["max_subissues_per_parent"], 10);
    assert_eq!(raw["board"]["budget"]["max_total_redispatch"], 5);
    assert_eq!(raw["board"]["budget"]["wall_clock_budget_secs"], 3600);
    assert_eq!(raw["board"]["review"]["checkers"], 3);
    assert_eq!(raw["board"]["budget"]["max_tokens_per_parent"], 100000);
    assert_eq!(raw["board"]["budget"]["max_send_retries"], 7);
    // typed 重序列化 round-trip：既有键不丢（P1 键仍在）。
    assert_eq!(raw["board"]["unlimited_mode"], false);

    // 未知嵌套键 loud 拒绝，错误信息列出允许键（含新键）。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.bogus", "value": true }),
    )
    .await
    .expect_err("未知 review 子键必须 loud 拒绝");
    assert!(
        err.contains("未知或不允许") && err.contains("review.max_turns"),
        "got: {err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------
// 停车场复活（A1/A2/B1-B5，2026-09-11）：tags 匹配、sweep 重试、⏸ 去重、
// 父单受阻联动、级联取消、无派发取消。
// ------------------------------------------------------------------

/// 独立 workspace 的离线 Cluster（不 start()，纯注册表/任务账本内存态；
/// peers.toml 落测试目录不污染源码树）。
#[cfg(feature = "cluster")]
fn offline_cluster(dir: &std::path::Path, node_id: &str) -> Arc<nemesis_cluster::cluster::Cluster> {
    Arc::new(nemesis_cluster::cluster::Cluster::with_workspace(
        nemesis_cluster::types::ClusterConfig {
            node_id: node_id.to_string(),
            bind_address: "127.0.0.1:0".into(),
            peers: vec![],
            node_name: String::new(),
        },
        dir.join("ws"),
    ))
}

/// 派发同步断言用的「哑对端」地址：进程内 loopback listener（永不 accept/
/// 响应，进程存活期常驻）。connect 在 OS backlog 即刻完成、ACK 永不到来
/// ——派发 spawn 内的 send 挂在外层超时上，同步断言窗口内 claim 行保持
/// 在途（不可路由地址的 connect 失败快慢随主机路由环境漂移，竞态不可
/// 接受；自备端口不进固定端口表）。
#[cfg(feature = "cluster")]
fn silent_peer_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind silent peer");
    let addr = listener.local_addr().expect("local addr");
    std::mem::forget(listener); // 常驻：close 会让后续 connect 立刻被拒
    format!("127.0.0.1:{}", addr.port())
}

/// 节点 → 哑对端地址的静态 [`PeerResolver`]（裸 `RpcClient::new()` 无
/// resolver，send 秒败 "peer unresolvable" → 回退路径抢跑——同步断言
/// 竞态；本解析器让 send 真解析到哑 listener，ACK 挂外层超时）。
#[cfg(feature = "cluster")]
struct TestStaticResolver {
    node_id: String,
    targets: std::collections::HashMap<String, String>,
}

#[cfg(feature = "cluster")]
impl nemesis_cluster::rpc::client::PeerResolver for TestStaticResolver {
    fn get_peer_info(&self, peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        self.targets
            .get(peer_id)
            .map(|a| (vec![a.clone()], 0, true))
    }
    fn get_local_interfaces(&self) -> Vec<nemesis_cluster::rpc::client::LocalNetworkInterface> {
        Vec::new()
    }
    fn get_node_id(&self) -> String {
        self.node_id.clone()
    }
}

/// 注入「可解析到哑对端」的 RPC client（map：节点 id → silent listener）。
#[cfg(feature = "cluster")]
fn inject_silent_peer_client(
    cluster: &nemesis_cluster::cluster::Cluster,
    targets: &[(&str, String)],
) {
    let client =
        nemesis_cluster::rpc::client::RpcClient::with_resolver(Arc::new(TestStaticResolver {
            node_id: cluster.node_id().to_string(),
            targets: targets
                .iter()
                .map(|(id, a)| (id.to_string(), a.clone()))
                .collect(),
        }));
    cluster.set_rpc_client(Arc::new(client));
}

#[cfg(feature = "cluster")]
fn planner_child(
    store: &Arc<BoardStore>,
    parent_id: i64,
    title: &str,
    tags: Vec<String>,
) -> nemesis_board::Issue {
    store
        .create_issue(nemesis_board::NewIssue {
            title: title.to_string(),
            parent_issue_id: Some(parent_id),
            required_tags: tags,
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: "NB-1".to_string(),
            }),
            ..Default::default()
        })
        .unwrap()
}

/// B4+B5：停车留痕去重 + 父单转受阻 + sweep 静默重试不刷评论。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn park_notice_dedup_parent_blocked_and_silent_sweep() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("park-dedup-block");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-1");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "子A(planner)", vec![]);
    // 手工单：无 origin 也无暂缓标记 → 不进 sweep 候选。
    let b = store
        .create_issue(nemesis_board::NewIssue {
            title: "子B(手工)".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();

    // 无匹配节点停车：⏸ 评论 + 父单 backlog → blocked。
    assert_eq!(
        super::dispatch_subissue_auto(&store, Some(&cluster), a.id, &actor, true).unwrap(),
        None
    );
    let notices = |id: i64| -> usize {
        store
            .list_comments(id)
            .unwrap()
            .iter()
            .filter(|c| c.content.contains("自动派发暂缓"))
            .count()
    };
    assert_eq!(notices(a.id), 1, "首次停车必须留痕");
    assert_eq!(notices(parent.id), 1, "父单受阻必须留痕");
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::Blocked,
        "整条链都在等节点 → 父单 blocked 显形"
    );

    // B4 去重：再次停车（notify=true）不堆叠同文案评论。
    super::dispatch_subissue_auto(&store, Some(&cluster), a.id, &actor, true).unwrap();
    assert_eq!(notices(a.id), 1, "同状态重复停车不得刷评论");
    assert_eq!(notices(parent.id), 1);

    // sweep 候选粗筛：planner 来源的 a 在列，手工 b 不在。
    let cands = store.list_dispatch_park_candidates().unwrap();
    assert!(cands.contains(&a.id), "planner 单必须是 sweep 候选");
    assert!(!cands.contains(&b.id), "无标记手工单不得进候选");

    // A2 静默重试：无匹配节点 → 派不出但零副作用（不刷评论不动父单）。
    let (cands, dispatched, failed) = super::sweep_parked_dispatches(&store, &cluster, &actor);
    assert_eq!(dispatched, 0);
    assert_eq!(failed, 0);
    assert!(cands >= 1);
    assert_eq!(notices(a.id), 1, "sweep 静默路径不得刷评论");
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::Blocked
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A2 金路径 + A1 标签匹配：节点带 tags 上线 → sweep 复活停车场单，
/// 父单 blocked → in_progress。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn sweep_redispatches_when_matching_tags_peer_appears() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("sweep-revive");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-2");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "子A", vec!["python".to_string()]);

    // 初始停车（空注册表）。
    super::dispatch_subissue_auto(&store, Some(&cluster), a.id, &actor, true).unwrap();
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Backlog);

    // 离线 Cluster 没有 RPC client（start() 才装）——派发核心需要它拿
    // fire-and-forget 发送句柄；注入空 client（对 127.0.0.1:19999 的
    // connect 失败走 spawn 内的 FAILED 终结，不影响本测试断言的同步面）。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    // 节点带 tags=["python"] 上线（生产路径 = merge_real_node_info）。
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-py".into(),
        name: "PyWorker".into(),
        address: "127.0.0.1:19999".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["python".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let (cands, dispatched, failed) = super::sweep_parked_dispatches(&store, &cluster, &actor);
    assert_eq!(failed, 0, "匹配成功不得有失败：{cands} 候选");
    assert_eq!(dispatched, 1, "tags 匹配的停车单必须被 sweep 派出");
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::InProgress,
        "派出后子单 in_progress"
    );
    assert!(
        store.has_active_dispatch(a.id).unwrap(),
        "派出后必须有在途派发记录"
    );
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::InProgress,
        "父单从 blocked 复活回 in_progress"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-U3-4：周期兜底 ticker 的 notify 路径——首次 sweep 停车即落 ⏸ 评论 +
/// 父单 blocked 显形（与直派路径同语义；此前 announce 路径静默，停车零
/// 反馈），重复 tick B4 去重不刷屏，匹配节点上线后 sweep 派出并解锁父单。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn periodic_sweep_notify_parks_with_notice_then_revives() {
    let dir = unique_dir("park-ticker-notify");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-ticker");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "子A", vec!["rust".to_string()]);

    // ticker 首拍：无匹配节点 → 停车留痕（notify=true）。
    let (cands, dispatched, failed) =
        super::sweep_parked_dispatches_notify(&store, &cluster, &actor, true);
    assert_eq!(cands, 1);
    assert_eq!(dispatched, 0);
    assert_eq!(failed, 0);
    let notices = |id: i64| -> usize {
        store
            .list_comments(id)
            .unwrap()
            .iter()
            .filter(|c| c.content.contains("自动派发暂缓"))
            .count()
    };
    assert_eq!(notices(a.id), 1, "ticker 路径首次停车必须留痕");
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::Blocked,
        "整条链在等节点 → 父单 blocked 显形"
    );

    // 重复 tick：B4 去重，不刷评论不动父单。
    let (cands, dispatched, failed) =
        super::sweep_parked_dispatches_notify(&store, &cluster, &actor, true);
    assert_eq!((cands, dispatched, failed), (1, 0, 0));
    assert_eq!(notices(a.id), 1, "重复 tick 不得刷评论");
    assert_eq!(notices(parent.id), 1);
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::Blocked
    );

    // 节点带 tags 上线 → sweep 派出 + 父单复活。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-rs".into(),
        name: "RsWorker".into(),
        address: "127.0.0.1:19997".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["rust".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    let (cands, dispatched, failed) =
        super::sweep_parked_dispatches_notify(&store, &cluster, &actor, true);
    assert_eq!(failed, 0);
    assert_eq!(dispatched, 1, "匹配节点上线后 ticker sweep 必须派出");
    assert_eq!(cands, 1);
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::InProgress
    );
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::InProgress,
        "父单从 blocked 复活"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// B3+B2：无在途派发的单取消不再拒绝；落定联动父单收口。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_cancel_without_dispatch_direct_terminal_and_parent_sync() {
    let dir = unique_dir("cancel-no-dispatch");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "子A".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": a.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], true);
    assert!(
        out.get("task_id").is_none() || out["task_id"].is_null(),
        "无派发取消不得编造 task_id"
    );
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::Cancelled
    );
    // 父单联动：任一子单 cancelled → in_review + 缺口评论。
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview
    );
    assert!(
        store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("未完成项")),
        "父单收口必须留缺口评论"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// B1：依赖取消后 dependents 原本是永久死链（依赖闸只认 done + 无
/// reopen）——现在级联取消让死链显形：B/孙单全部 cancelled + ⛔ 留痕。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn cancel_cascades_to_dependents() {
    let dir = unique_dir("cancel-cascade");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "子A".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let b = store
        .create_issue(nemesis_board::NewIssue {
            title: "子B".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let g = store
        .create_issue(nemesis_board::NewIssue {
            title: "孙G".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    store.set_issue_dependencies(b.id, &[a.id]).unwrap();
    store.set_issue_dependencies(g.id, &[b.id]).unwrap();
    // 终态单不被级联波及。
    let done = store
        .create_issue(nemesis_board::NewIssue {
            title: "已完成".into(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(done.id, IssueStatus::Done, &actor)
        .unwrap();

    // 无在途派发直接取消 a → b、g 级联。
    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": a.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], true);

    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::Cancelled
    );
    assert_eq!(
        store.get_issue(b.id).unwrap().status,
        IssueStatus::Cancelled,
        "直接依赖必须级联取消"
    );
    assert_eq!(
        store.get_issue(g.id).unwrap().status,
        IssueStatus::Cancelled,
        "传递依赖（孙单）必须级联取消"
    );
    assert_eq!(
        store.get_issue(done.id).unwrap().status,
        IssueStatus::Done,
        "终态单不被级联波及"
    );
    for id in [b.id, g.id] {
        assert!(
            store
                .list_comments(id)
                .unwrap()
                .iter()
                .any(|c| c.content.contains("级联取消")),
            "级联单必须 ⛔ 留痕"
        );
    }
    // C3：cancel 响应体必须披露级联清单（不再无声）。
    let cascaded: Vec<&str> = out["cascade_cancelled"]
        .as_array()
        .expect("响应必须带 cascade_cancelled 数组")
        .iter()
        .map(|v| v.as_str().expect("级联清单元素应为编号字符串"))
        .collect();
    assert_eq!(cascaded.len(), 2, "恰好 b、g 两单被连带：{cascaded:?}");
    assert!(cascaded.contains(&b.number.as_str()), "{cascaded:?}");
    assert!(cascaded.contains(&g.number.as_str()), "{cascaded:?}");

    // ④ reopen 通路：级联取消的单可用 issue.reopen 拉回 backlog（不再
    // SQL 手术），审计评论落盘；reopen 后依赖闸重算（b 依赖 a——a 已
    // 复活为 backlog 非 done，b 保持待派）。
    for id in [a.id, b.id, g.id] {
        let out = dispatch(&ctx, "issue.reopen", serde_json::json!({ "id": id }))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out["reopened"], true);
        let issue = store.get_issue(id).unwrap();
        assert_eq!(issue.status, IssueStatus::Backlog);
        assert!(
            store
                .list_comments(id)
                .unwrap()
                .iter()
                .any(|c| c.ctype == nemesis_board::CommentType::System
                    && c.content.contains("reopen")),
            "reopen 必须落系统审计评论"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-U4-3：cancel **父单**必须经父子边级联取消子单——planner 拆解的依赖
/// 边是兄弟链、从不指向父单，只走依赖边则父单取消恒零级联（真实 UAT
/// 事故：三子单僵尸 backlog + 补派触发器还会继续派它们空烧 token）。
/// 在途派发的子单连带终结派发行（竞态守卫赢），披露清单含全部子单。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn cancel_parent_cascades_children_via_parent_edge() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("cancel-parent-cascade");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let parent = no_parent(&store);
    // 三子单只有 parent_issue_id 关联（U4-3 真实夹具：零依赖边）。
    let c1 = planner_child(&store, parent, "子甲", vec![]);
    let c2 = planner_child(&store, parent, "子乙", vec![]);
    let c3 = planner_child(&store, parent, "子丙", vec![]);

    // 前置：子乙有在途派发（在线 peer 派出——对应真实 UAT 的 worker 执行中）。
    let cluster = offline_cluster(&dir, "coord-pc1");
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-w1".into(),
        name: "W1".into(),
        address: "127.0.0.1:19996".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        Some(&cluster),
        c2.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(
        store.has_active_dispatch(c2.id).unwrap(),
        "前置：子乙必须有在途派发"
    );

    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": parent }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], true);
    assert_eq!(
        store.get_issue(parent).unwrap().status,
        IssueStatus::Cancelled
    );
    for (id, label) in [(c1.id, "子甲"), (c2.id, "子乙"), (c3.id, "子丙")] {
        assert_eq!(
            store.get_issue(id).unwrap().status,
            IssueStatus::Cancelled,
            "{label} 必须随父单级联取消"
        );
        assert!(
            store
                .list_comments(id)
                .unwrap()
                .iter()
                .any(|c| c.content.contains("级联取消")),
            "{label} 必须 ⛔ 留痕"
        );
    }
    assert!(
        !store.has_active_dispatch(c2.id).unwrap(),
        "子乙的在途派发必须连带终结（worker 不再空烧）"
    );
    let cascaded: Vec<&str> = out["cascade_cancelled"]
        .as_array()
        .expect("响应必须披露级联清单")
        .iter()
        .map(|v| v.as_str().expect("级联清单元素应为编号"))
        .collect();
    assert_eq!(cascaded.len(), 3, "三子单全在披露清单：{cascaded:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-U4-3 派发侧闸：父单已取消的子单不再被派出（补派触发器/停车场
/// sweep 都过这道闸）——否则父单死后子单还会被派发空烧 token。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn dispatch_skips_children_of_cancelled_parent() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("dispatch-dead-parent");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-pc2");
    let parent = no_parent(&store);
    let child = planner_child(&store, parent, "子A", vec!["python".to_string()]);
    // 父单落 cancelled（直接终态：无在途派发不需要集群）。
    store
        .transition_issue(parent, IssueStatus::Cancelled, &actor)
        .unwrap();

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        Some(&cluster),
        child.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_none(), "父单已取消的子单不得派出");
    assert_eq!(
        store.get_issue(child.id).unwrap().status,
        IssueStatus::Backlog
    );
    assert!(!store.has_active_dispatch(child.id).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-U4-3 reopen 侧闸：父单已取消的子单不可单独复活（否则被派发闸拦成
/// 永久 backlog 僵尸）——先复活父单，再逐单复活子单。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn reopen_refuses_child_of_cancelled_parent() {
    let dir = unique_dir("reopen-dead-parent");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let parent = no_parent(&store);
    let child = planner_child(&store, parent, "子A", vec![]);
    store
        .transition_issue(child.id, IssueStatus::Cancelled, &actor)
        .unwrap();
    store
        .transition_issue(parent, IssueStatus::Cancelled, &actor)
        .unwrap();

    let err = dispatch(&ctx, "issue.reopen", serde_json::json!({ "id": child.id }))
        .await
        .unwrap_err();
    assert!(
        err.contains("请先 reopen 父单"),
        "必须诚实拒绝并指路：{err}"
    );
    // 先复活父单 → 子单 reopen 放行。
    dispatch(&ctx, "issue.reopen", serde_json::json!({ "id": parent }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.get_issue(parent).unwrap().status,
        IssueStatus::Backlog
    );
    dispatch(&ctx, "issue.reopen", serde_json::json!({ "id": child.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.get_issue(child.id).unwrap().status,
        IssueStatus::Backlog
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ④ reopen（发现④ 2026-09-15）：非法来源 loud 拒绝（todo/done 不可走
/// reopen），cancelled → backlog 走通且 has_active_dispatch 天然解锁
///（无在途派发记录），可直接重新 dispatch（此处验证表面契约：reopen 后
/// 再 cancel 不再被旧状态卡死）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_reopen_rejects_non_cancelled_and_restores_dispatchability() {
    let dir = unique_dir("issue-reopen");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "待复活".into(),
            ..Default::default()
        })
        .unwrap();

    // 非取消来源 loud 拒绝。
    let err = dispatch(&ctx, "issue.reopen", serde_json::json!({ "id": a.id }))
        .await
        .unwrap_err();
    assert!(err.contains("只有已取消"), "{err}");

    // cancelled → backlog。
    store
        .transition_issue(a.id, IssueStatus::Cancelled, &actor)
        .unwrap();
    let out = dispatch(&ctx, "issue.reopen", serde_json::json!({ "id": a.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["reopened"], true);
    assert_eq!(out["issue"]["status"], "backlog");
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::Backlog,
        "reopen 后单子回 backlog"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// B5 收尾路径：受阻父单的子单全部落定（含取消）→ 先垫 in_progress 再
/// in_review（blocked→in_review 非法，垫底条件必须含 blocked）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn sync_parent_status_bumps_blocked_parent_before_review() {
    let dir = unique_dir("parent-blocked-bump");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let p = store
        .create_issue(nemesis_board::NewIssue {
            title: "受阻父单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(p.id, IssueStatus::Blocked, &actor)
        .unwrap();
    let c = store
        .create_issue(nemesis_board::NewIssue {
            title: "子".into(),
            parent_issue_id: Some(p.id),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(c.id, IssueStatus::Done, &actor)
        .unwrap();
    super::sync_parent_status(&store, p.id, &actor).unwrap();
    assert_eq!(
        store.get_issue(p.id).unwrap().status,
        IssueStatus::InReview,
        "blocked 父单全子落定必须经 in_progress 垫底进 in_review"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------
// 停车场兜底派发开关（集群完备性加固 2026-09-11）：dispatch_fallback /
// dispatch_fallback_target。无人匹配时任务必须能推进（用户裁决）。
// ------------------------------------------------------------------

/// 构造兜底档位配置（显式入参，测试不碰进程级全局 live config）。
#[cfg(feature = "cluster")]
fn fallback_cfg(
    mode: DispatchFallbackMode,
    target: Option<&str>,
) -> nemesis_config::BoardFlagConfig {
    nemesis_config::BoardFlagConfig {
        dispatch_fallback: mode,
        dispatch_fallback_target: target.map(str::to_string),
        ..Default::default()
    }
}

/// 无父单工具：直接建一个顶层单（planner 来源，进 sweep 候选集）。
#[cfg(feature = "cluster")]
fn no_parent(store: &Arc<BoardStore>) -> i64 {
    store
        .create_issue(nemesis_board::NewIssue {
            title: "父占位".into(),
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: "NB-1".to_string(),
            }),
            ..Default::default()
        })
        .unwrap()
        .id
}

/// 开关关（默认）：无人匹配 → 原 ⏸ 停车语义原样保留（回归锚）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_off_parks_as_before() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-off");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb0");
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Off, None)),
        &store,
        Some(&cluster),
        a.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_none(), "开关关不得兜底派出");
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Backlog);
    assert!(
        store
            .last_system_comment(a.id)
            .unwrap()
            .map(|c| c.contains("⏸"))
            .unwrap_or(false),
        "必须落 ⏸ 停车评论"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 开关开 + 无任何在线节点 → 仍诚实停车（兜底造不出客户端）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_on_but_no_peers_still_parks() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-no-peer");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb1");
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        Some(&cluster),
        a.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_none(), "无在线节点不得凭空派发");
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Backlog);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Role 档（新默认）角色纪律保留：coordinator 单只有 worker 在线——④全
/// 松弛才能救的局诚实停车；Full 档同场景才兜底派出（2026-09-29 三档化
/// 回归锚：默认不越 worker/coordinator 纪律）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_role_depth_keeps_role_discipline() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-role-depth");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb-rd");
    let parent = no_parent(&store);
    // coordinator 单：只有④（角色放开）能救。
    let c = store
        .create_issue(nemesis_board::NewIssue {
            title: "协调子单".into(),
            parent_issue_id: Some(parent),
            required_role: Some("coordinator".to_string()),
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: "NB-1".to_string(),
            }),
            ..Default::default()
        })
        .unwrap();

    // 上线一个 worker 节点（无任何标签/职能——只有④可达）。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-w".into(),
        name: "W1".into(),
        address: "127.0.0.1:19997".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // Role 档（新默认）：不越角色纪律 → 诚实停车 + ⏸ 评论。
    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Role, None)),
        &store,
        Some(&cluster),
        c.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_none(), "Role 档不得把 coordinator 单兜给 worker");
    assert_eq!(store.get_issue(c.id).unwrap().status, IssueStatus::Backlog);

    // Full 档：角色放开 → 兜底派出。
    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        Some(&cluster),
        c.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_some(), "Full 档应全松弛兜底派出");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 开关开 + 在线节点不带 python：松弛兜底派出 + ⚠ 评论留痕；父单不转
/// blocked（首派即走，blocked 联动只属于停车路径）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_dispatches_to_relaxed_online_peer() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-relaxed");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb2");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "子A", vec!["python".to_string()]);

    // 上线节点不带 python（tags=["rust"]）——严格匹配必空。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-rs".into(),
        name: "RsWorker".into(),
        address: "127.0.0.1:19998".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["rust".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        Some(&cluster),
        a.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_some(), "兜底必须派出（任务做下去）");
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::InProgress
    );
    assert!(store.has_active_dispatch(a.id).unwrap());
    let comments = store.list_comments(a.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains('⚠') && c.content.contains("node-rs")),
        "兜底派发必须落 ⚠ 评论并指明目标节点：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::InProgress,
        "父单随首派进 in_progress（不走 blocked）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 钉住目标：按 name 匹配在线节点派发；钉住的节点不在线 = 诚实停车。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_pinned_target_name_match_and_offline_honesty() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-pin");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb3");
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-alex".into(),
        name: "Alex".into(),
        address: "127.0.0.1:19997".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 钉住 name（大小写不敏感）→ 派给该节点。
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);
    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, Some("alex"))),
        &store,
        Some(&cluster),
        a.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_some(), "钉住 name 命中在线节点必须派出");
    let dispatch = store
        .list_dispatches(a.id)
        .unwrap()
        .into_iter()
        .next()
        .expect("派出必须有派发记录");
    assert_eq!(dispatch.worker_id, "node-alex", "必须派给钉住的节点");

    // 钉住的节点不在线（另一节点在线也不换人）→ 停车。
    let b = planner_child(&store, parent, "子B", vec!["python".to_string()]);
    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, Some("Ghost"))),
        &store,
        Some(&cluster),
        b.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_none(), "钉住目标不在线必须诚实停车");
    assert_eq!(store.get_issue(b.id).unwrap().status, IssueStatus::Backlog);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 角色松弛次序：required_role=worker 的单在仅 coordinator 在线时全放开
/// 兜底仍能派出（任务做下去优先），且评论说明「全松弛」。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_role_relaxed_ordering() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-role");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb4");
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    // 仅 coordinator 对端在线。
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-co".into(),
        name: "CoPeer".into(),
        address: "127.0.0.1:19996".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Coordinator,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let parent = no_parent(&store);
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "子A".into(),
            parent_issue_id: Some(parent),
            required_role: Some("worker".to_string()),
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: "NB-1".to_string(),
            }),
            ..Default::default()
        })
        .unwrap();
    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        Some(&cluster),
        a.id,
        &actor,
        true,
    )
    .unwrap();
    assert!(out.is_some(), "全松弛兜底必须派出");
    let comments = store.list_comments(a.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("全松弛")),
        "全松弛兜底必须留痕说明"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// sweep 携带同一兜底配置：存量停车单被兜底复活（announce 触发路径）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_sweep_revives_parked_issue() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("fallback-sweep");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb5");

    // 开关关时先停车。
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);
    super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Off, None)),
        &store,
        Some(&cluster),
        a.id,
        &actor,
        true,
    )
    .unwrap();
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Backlog);

    // 节点上线（仍无 python）+ 开关打开 → sweep 兜底复活。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-any".into(),
        name: "AnyWorker".into(),
        address: "127.0.0.1:19995".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "general".into(),
        capabilities: vec![],
        tags: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    let (cands, dispatched, failed) = super::sweep_parked_dispatches_with_config(
        Some(&fallback_cfg(DispatchFallbackMode::Full, None)),
        &store,
        &cluster,
        &actor,
        false,
    );
    assert_eq!(failed, 0);
    assert_eq!(dispatched, 1, "兜底开时 sweep 必须复活停车单");
    assert_eq!(cands, 1);
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::InProgress
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P1 拓扑硬闸（2026-09-12 双端真机 S2 根修）：远端目标 + file: 锚点 = 拒绝派发
// （被测 fn 本体 cluster-gated → 测试同守卫，无 feature 编译期消失）
// ---------------------------------------------------------------------------

#[test]
#[cfg(feature = "cluster")]
fn remote_file_anchor_gate_rejects_remote_target() {
    let ac = "文件存在\n[CHECK] file:deploy_demo/index.html exists\n[CHECK] re:已创建";
    let reason = super::reject_remote_file_anchors(Some(ac), "Node-B", "node-a-xyz", "Node-A")
        .expect("远端目标 + file: 锚点必须拒绝");
    assert!(reason.contains("拒绝派发"), "拒绝理由需说明闸门：{reason}");
    assert!(
        reason.contains("Node-B"),
        "拒绝理由需点名目标节点：{reason}"
    );
    assert!(
        reason.contains("file:deploy_demo/index.html exists"),
        "拒绝理由需回显锚点原文：{reason}"
    );
    assert!(
        reason.contains("re:"),
        "拒绝理由需给出 re: 修正建议：{reason}"
    );
}

#[test]
#[cfg(feature = "cluster")]
fn remote_file_anchor_gate_allows_content_regex_only() {
    let ac = "[CHECK] re:已创建 index[.]html\n普通语义验收行";
    assert!(
        super::reject_remote_file_anchors(Some(ac), "Node-B", "node-a-xyz", "Node-A").is_none(),
        "纯 re: 锚点跨节点安全，不得拦截"
    );
}

#[test]
#[cfg(feature = "cluster")]
fn remote_file_anchor_gate_allows_local_target_by_id_and_name() {
    let ac = "[CHECK] file:deploy_demo/index.html exists";
    // 按 id 命中本节点。
    assert!(
        super::reject_remote_file_anchors(Some(ac), "node-a-xyz", "node-a-xyz", "Node-A").is_none()
    );
    // 按名称命中本节点。
    assert!(
        super::reject_remote_file_anchors(Some(ac), "Node-A", "node-a-xyz", "Node-A").is_none()
    );
    // 大小写不敏感。
    assert!(
        super::reject_remote_file_anchors(Some(ac), "node-a", "node-a-xyz", "Node-A").is_none()
    );
}

#[test]
#[cfg(feature = "cluster")]
fn remote_file_anchor_gate_no_criteria_or_no_file_anchor() {
    assert!(super::reject_remote_file_anchors(None, "Node-B", "node-a-xyz", "Node-A").is_none());
    assert!(
        super::reject_remote_file_anchors(Some(""), "Node-B", "node-a-xyz", "Node-A").is_none()
    );
    // 混合形态：有 file: 就拦（file: 是误杀源，re: 不救）。
    let mixed = "[CHECK] re:交付完成\n[CHECK] file:info.txt contains:kangjinlong";
    assert!(
        super::reject_remote_file_anchors(Some(mixed), "Node-B", "node-a-xyz", "Node-A").is_some()
    );
}

#[test]
#[cfg(feature = "cluster")]
fn remote_file_anchor_gate_ignores_unsafe_path_anchors() {
    // 路径形态不安全的 file: 锚点（绝对路径）在 parse_anchors 已被拒收
    // （回落语义项 + 告警）——不触发派发闸（那是验收标准自身的毛病，
    // review 侧已有降级路径，不在派发面重复惩罚）。
    let ac = r"[CHECK] file:C:\evil\x exists";
    assert!(
        super::reject_remote_file_anchors(Some(ac), "Node-B", "node-a-xyz", "Node-A").is_none()
    );
}

// ---------------------------------------------------------------------------
// goal P1/C1+C2：board.project.progress 项目进度聚合
// （环节计数/卡点环节/逐单环节数据——实机验收=goal §四 T-obs-2/3/4）。
// ---------------------------------------------------------------------------

#[test]
fn test_project_progress_counts_stage_and_rows() {
    let dir = unique_dir("project-progress");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let actor = nemesis_board::Actor::admin("test-session");
    let project = store
        .create_project("进度测试", "d", None, "", "", None)
        .unwrap();

    let new = |title: &str| nemesis_board::NewIssue {
        title: title.into(),
        project_id: Some(project.id),
        ..Default::default()
    };
    let done = store.create_issue(new("done 单")).unwrap();
    let review = store.create_issue(new("验收中单")).unwrap();
    let redo = store.create_issue(new("打回重做单")).unwrap();
    let running = store.create_issue(new("执行中单")).unwrap();
    let parked = store.create_issue(new("停车单")).unwrap();
    let _ready = store.create_issue(new("待派发单")).unwrap();
    let blocked = store.create_issue(new("受阻单")).unwrap();
    let cancelled = store.create_issue(new("取消单")).unwrap();

    // 各单推到目标状态（转移边均走 state_machine 合法路径）。
    store
        .transition_issue(done.id, nemesis_board::models::IssueStatus::Done, &actor)
        .unwrap();
    // 验收中：backlog → in_progress → in_review（backlog 无直达 in_review 边）。
    store
        .transition_issue(
            review.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    store
        .transition_issue(
            review.id,
            nemesis_board::models::IssueStatus::InReview,
            &actor,
        )
        .unwrap();
    // 打回重做：in_review → in_progress（合法边），且不插在途派发 → 待重派。
    store
        .transition_issue(
            redo.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    store
        .transition_issue(
            running.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    store
        .insert_dispatch("task-running", running.id, "node-x", &actor)
        .unwrap();
    // 停车：backlog + ⏸ 系统评论（PARK_NOTICE_MARK 同源，sweep 判定一致）。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: parked.id,
            author: nemesis_board::Actor::system("board"),
            content: format!(
                "⏸ {}：当前在线节点无可匹配（角色/标签）",
                super::PARK_NOTICE_MARK
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    store
        .transition_issue(
            blocked.id,
            nemesis_board::models::IssueStatus::Blocked,
            &actor,
        )
        .unwrap();
    store
        .transition_issue(
            cancelled.id,
            nemesis_board::models::IssueStatus::Cancelled,
            &actor,
        )
        .unwrap();

    let out = super::project_progress(&store, project.id).unwrap();
    assert_eq!(out["total"], 8);
    let counts = &out["counts"];
    assert_eq!(counts["done"], 1);
    assert_eq!(counts["dispatched"], 1);
    assert_eq!(
        counts["in_progress"], 1,
        "打回重做（无在途）单计 in_progress"
    );
    assert_eq!(counts["in_review"], 1);
    assert_eq!(counts["parked"], 1);
    assert_eq!(counts["backlog"], 1);
    assert_eq!(counts["blocked"], 1);
    assert_eq!(counts["cancelled"], 1);
    // 卡点优先级：受阻 > 停车 > 执行中 > 待重派 > 验收中 > 待派发
    assert_eq!(out["stage"], "受阻");

    // 逐单环节（C2 徽标数据源）
    let stage_of = |title: &str| -> String {
        out["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["title"].as_str() == Some(title))
            .map(|r| r["stage"].as_str().unwrap().to_string())
            .unwrap_or_default()
    };
    assert_eq!(stage_of("done 单"), "已完成");
    assert_eq!(stage_of("验收中单"), "验收中");
    assert_eq!(stage_of("打回重做单"), "待重派");
    assert_eq!(stage_of("执行中单"), "执行中");
    assert_eq!(stage_of("停车单"), "已停车");
    assert_eq!(stage_of("待派发单"), "待派发");
    assert_eq!(stage_of("受阻单"), "受阻");
    assert_eq!(stage_of("取消单"), "已取消");
}

#[test]
fn test_project_progress_empty_and_stage_priority() {
    let dir = unique_dir("project-progress-empty");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let actor = nemesis_board::Actor::admin("test-session");
    let project = store
        .create_project("空项目", "", None, "", "", None)
        .unwrap();

    // 空项目 → 未拆解
    let out = super::project_progress(&store, project.id).unwrap();
    assert_eq!(out["total"], 0);
    assert_eq!(out["stage"], "未拆解");

    // 卡点优先级：停车 > 执行中（混合场景 stage 取更靠前的卡点）
    let parked = store
        .create_issue(nemesis_board::NewIssue {
            title: "停车单".into(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: parked.id,
            author: nemesis_board::Actor::system("board"),
            content: format!("⏸ {}：无可匹配", super::PARK_NOTICE_MARK),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    let running = store
        .create_issue(nemesis_board::NewIssue {
            title: "执行中单".into(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(
            running.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    store
        .insert_dispatch("task-prio", running.id, "node-x", &actor)
        .unwrap();

    let out = super::project_progress(&store, project.id).unwrap();
    assert_eq!(out["stage"], "停车待恢复");
}

// ---------------------------------------------------------------------------
// goal P2/D0：派发准入控制（worker_max_inflight）纯函数判定。
// （自动链集成路径由 cluster-uat 双 worker 场景覆盖——本处钉判定语义。）
// ---------------------------------------------------------------------------

#[test]
fn test_inflight_full_semantics() {
    let mut load = std::collections::HashMap::new();
    load.insert("node-x".to_string(), 1usize);
    load.insert("node-y".to_string(), 3usize);

    // cap=1：node-x 在途 1 ≥ 1 = 满；node-y 在途 3 同样满；node-z 不在
    // load（0 在途）= 不满。
    assert!(super::inflight_full(&load, "node-x", 1));
    assert!(super::inflight_full(&load, "node-y", 1));
    assert!(!super::inflight_full(&load, "node-z", 1));

    // cap=0 = 不限（旧行为）：再满也不拦。
    assert!(!super::inflight_full(&load, "node-x", 0));

    // cap=3：node-y 恰好触顶。
    assert!(super::inflight_full(&load, "node-y", 3));

    // cap 负数防御 = 视同不限（构造期已过滤，此处双保险）。
    assert!(!super::inflight_full(&load, "node-x", -1));
}

// ---------------------------------------------------------------------------
// goal P2/P5-E：标签授予台账（board_meta granted_tags）。
// ---------------------------------------------------------------------------

#[test]
fn test_grant_tags_to_node_store() {
    let dir = unique_dir("grant-tags");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");

    // 首次授予：python + web。
    let g1 = store
        .grant_tags_to_node("node-b", &["python".to_string(), "web".to_string()])
        .unwrap();
    assert_eq!(g1, vec!["python", "web"]);

    // 重复授予 → 不重复（返回空）。
    let g2 = store
        .grant_tags_to_node("node-b", &["python".to_string()])
        .unwrap();
    assert!(g2.is_empty());

    // 台账查询：node_id → tags。
    let map = store.granted_tags_map().unwrap();
    assert_eq!(
        map.get("node-b").unwrap(),
        &vec!["python".to_string(), "web".to_string()]
    );
}

// ---------------------------------------------------------------------------
// goal P2/D0b 回归：running 态派发的终结与在途判定（NB-12 卡死类回归防线）。
// ---------------------------------------------------------------------------

#[test]
fn test_running_dispatch_lifecycle() {
    let dir = unique_dir("running-dispatch");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let actor = nemesis_board::Actor::admin("test-session");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "t".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("tk-r1", issue.id, "node-x", &actor)
        .unwrap();

    // 在途判定：dispatched 与 running 都算在途。
    assert!(store.has_active_dispatch(issue.id).unwrap());

    // 出队开跑上报 → running。
    assert!(store.mark_dispatch_running("tk-r1", "node-x").unwrap());
    // 重复上报 → false（已非 dispatched）。
    assert!(!store.mark_dispatch_running("tk-r1", "node-x").unwrap());
    assert!(
        store.has_active_dispatch(issue.id).unwrap(),
        "running 也算在途"
    );

    // finish：running → done 可终结。
    assert!(store.finish_dispatch("tk-r1", "done").unwrap());
    assert!(
        !store.has_active_dispatch(issue.id).unwrap(),
        "终结后不再在途"
    );
}

// ---------------------------------------------------------------------------
// D0 × T37 身份归一化回归（2026-09-13）：auto 派发 target 必须在 D0 闸之前
// 归一化为节点 id——人工指派给 peer 名（"Alex"）时，负载表（键=账本
// worker_id，归一化后全为节点 id）按名字查不到 = 闸失效 + 账本/上报身份
// 分裂（task.started / delivery.files 校验永假）。
// ---------------------------------------------------------------------------

#[cfg(feature = "cluster")]
#[tokio::test]
async fn inflight_gate_hits_canonical_worker_id_for_named_assignee() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("inflight-canonical");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-1");
    cluster.register_node(nemesis_cluster::types::ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-alex-1".into(),
            name: "Alex".into(),
            role: NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 占满 node-alex-1 的 slot：在途派发账本形态 = 节点 id（归一化后唯一形态）。
    let occupied = store
        .create_issue(nemesis_board::NewIssue {
            title: "占槽单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("tk-full", occupied.id, "node-alex-1", &actor)
        .unwrap();

    // 待派单：人工指派给 peer 名（非节点 id）。
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "名字指派单".into(),
            assignee: Some(nemesis_board::AssignmentType::Worker),
            assignee_id: Some("Alex".into()),
            ..Default::default()
        })
        .unwrap();

    let cfg = nemesis_config::BoardFlagConfig {
        worker_max_inflight: 1,
        ..Default::default()
    };
    let out = dispatch_subissue_auto_with_config(
        Some(&cfg),
        &store,
        Some(&cluster),
        issue.id,
        &actor,
        true,
    );
    assert!(
        matches!(out, Ok(None)),
        "peer 名指派必须经归一化命中负载表（node-alex-1 在途 1/1）→ 满仓停车"
    );
    // 停车评论按节点 id 留痕（身份口径统一）。
    let c = store
        .last_system_comment(issue.id)
        .unwrap()
        .expect("⏸ 评论必须落");
    assert!(
        c.contains("在途派发已达上限") && c.contains("node-alex-1"),
        "评论应含 ⏸ 标记 + 节点 id 形态目标，got: {c}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// R-9 互斥释放波（2026-09-13 T-sched-1 实机缺口 NB-18）：同父两 planner
/// 子单声明同写路径（[TOUCH] shared/a.txt）——冲突单静默延后留 backlog；
/// 冲突派发落定（finish_dispatch DONE，对应写回 settled=true）后，
/// sweep_parked_dispatches 必须把延后单承接派出。守卫「稳态集群冲突落定
/// 触发重估波」链路（gateway 回调点释放波的本体逻辑）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn mutex_deferred_single_dispatches_after_conflict_settles() {
    use nemesis_board::models::IssueStatus as St;
    use nemesis_board::models::dispatch_state;

    let dir = unique_dir("mutex-release-wave");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-mutex");

    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父".into(),
            ..Default::default()
        })
        .unwrap();
    // 两 planner 子单声明同写路径（互斥对）；ac 里另含 [TOUCH] 行。
    let mk_child = |title: &str| {
        store
            .create_issue(nemesis_board::NewIssue {
                title: title.to_string(),
                parent_issue_id: Some(parent.id),
                acceptance_criteria: Some("回复包含：完成\n[TOUCH] shared/a.txt".to_string()),
                origin: Some(nemesis_board::TaskOrigin {
                    origin_type: "planner".to_string(),
                    origin_id: "NB-1".to_string(),
                }),
                ..Default::default()
            })
            .unwrap()
    };
    let sub_a = mk_child("子A：写共享文件");
    let sub_b = mk_child("子B：同路径后写");

    // 离线 Cluster 注入空 RPC client + 单个 worker 上线。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-w1".into(),
        name: "W1".into(),
        address: "127.0.0.1:19999".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 子A 派出（在途）；子B 命中 R-9 互斥闸 → 静默延后留 backlog。
    let d = super::dispatch_subissue_auto(&store, Some(&cluster), sub_a.id, &actor, true).unwrap();
    assert!(d.is_some(), "首个无冲突单必须派出");
    assert_eq!(store.get_issue(sub_a.id).unwrap().status, St::InProgress);
    let out =
        super::dispatch_subissue_auto(&store, Some(&cluster), sub_b.id, &actor, true).unwrap();
    assert!(out.is_none(), "同路径冲突单必须被 R-9 闸静默延后");
    assert_eq!(
        store.get_issue(sub_b.id).unwrap().status,
        St::Backlog,
        "延后单保持待派态（停车场候选）"
    );

    // 冲突派发落定（= 写回 settled=true 的 store 层效果）。
    let disp = store
        .get_active_dispatch(sub_a.id)
        .unwrap()
        .expect("子A 必须有在途派发");
    assert!(
        store
            .finish_dispatch(&disp.task_id, dispatch_state::DONE)
            .unwrap()
    );
    let _ = store.transition_issue(sub_a.id, St::Done, &actor);

    // 释放波：sweep 必须把延后的子B 承接派出。
    let (cands, dispatched, failed) = super::sweep_parked_dispatches(&store, &cluster, &actor);
    assert_eq!(failed, 0, "释放波不得有失败：{cands} 候选");
    assert_eq!(dispatched, 1, "互斥释放后延后单必须被重估派出");
    assert_eq!(store.get_issue(sub_b.id).unwrap().status, St::InProgress);
    assert!(
        store.has_active_dispatch(sub_b.id).unwrap(),
        "子B 必须有新的在途派发"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------
// B1 匹配失败明细（2026-09-13 goal 复核补齐）：match_failure_detail
// 纯函数直测（函数注释声称「单测直测」但首轮落地遗漏测试）+
// project_resume 无匹配候选附 match_detail（goal P2/D 顺序依赖条款）。
// ------------------------------------------------------------------

#[cfg(feature = "cluster")]
fn cand(id: &str, role: &str, tags: &[&str]) -> nemesis_board::PeerCandidate {
    nemesis_board::PeerCandidate {
        id: id.into(),
        name: id.into(),
        role: role.into(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        capabilities: vec![],
        professions: vec![],
        tier: None,
        runtime_backend: None,
    }
}

/// 仅 required_* 影响 detail 的最小 Issue（直接构造，钉住纯函数语义）。
#[cfg(feature = "cluster")]
fn bare_issue(role: Option<&str>, tags: &[&str]) -> nemesis_board::Issue {
    nemesis_board::Issue {
        id: 1,
        number: "NB-1".into(),
        title: "t".into(),
        description: String::new(),
        status: IssueStatus::Backlog,
        priority: priority::MEDIUM,
        assignee: None,
        assignee_id: None,
        creator: Actor::system("board"),
        parent_issue_id: None,
        project_id: None,
        due_date: None,
        hidden: false,
        position: 0,
        acceptance_criteria: None,
        origin: None,
        required_role: role.map(str::to_string),
        required_tags: tags.iter().map(|s| s.to_string()).collect(),
        required_profession: None,
        required_backend: None,
        required_capabilities: vec![],
        requires_evidence: false,
        created_at: 0,
        updated_at: 0,
    }
}

#[cfg(feature = "cluster")]
#[test]
fn match_detail_reports_role_and_tag_gaps_per_candidate() {
    // 角色不匹配 + 缺标签：逐项列出。
    let d = super::match_failure_detail(
        &bare_issue(Some("worker"), &["rust", "web"]),
        &[cand("n1", "coordinator", &["web"])],
    );
    assert!(d.contains("✗"), "got: {d}");
    assert!(d.contains("role≠worker"), "got: {d}");
    assert!(d.contains("缺标签 rust"), "got: {d}");
    assert!(!d.contains("缺标签 web"), "已具备标签不得报缺：{d}");

    // 全命中：✓ 且无「缺:」段。
    let d = super::match_failure_detail(
        &bare_issue(Some("worker"), &["rust"]),
        &[cand("n1", "worker", &["rust", "extra"])],
    );
    assert!(d.contains("✓") && !d.contains("缺:"), "got: {d}");

    // 角色词表外（如 "rust"）转 tags 语义（与 rank_dispatch_candidates
    // 同源归一）：报缺标签而非 role≠。
    let d = super::match_failure_detail(
        &bare_issue(Some("rust"), &[]),
        &[cand("n1", "worker", &["web"])],
    );
    assert!(d.contains("缺标签 rust"), "got: {d}");
    assert!(!d.contains("role≠"), "got: {d}");

    // 多候选逐行分号连接，各自独立判定。
    let d = super::match_failure_detail(
        &bare_issue(None, &["rust"]),
        &[
            cand("n1", "worker", &["rust"]),
            cand("n2", "worker", &["web"]),
        ],
    );
    assert!(d.contains("；"), "got: {d}");
    assert!(d.contains("n1") && d.contains("n2"), "got: {d}");

    // 无在线候选。
    assert_eq!(
        super::match_failure_detail(&bare_issue(None, &[]), &[]),
        "无在线候选节点"
    );
}

/// B1×D 接线：project_resume 无匹配候选在 dry_run 预览行与执行失败行附
/// `match_detail`；匹配成功行不附。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn project_resume_no_match_carries_b1_detail() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("resume-b1-detail");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "node-master");

    let project = store
        .create_project("恢复明细", "", None, "", "", None)
        .unwrap();
    let child = store
        .create_issue(nemesis_board::NewIssue {
            title: "无匹配子单".into(),
            project_id: Some(project.id),
            required_tags: vec!["rust".into()],
            ..Default::default()
        })
        .unwrap();

    // 离线集群：无任何候选 → dry_run 预览行附「无在线候选节点」明细。
    let out = super::project_resume(&store, &cluster, None, project.id, true, &actor)
        .await
        .unwrap();
    let row = &out["candidates"][0];
    assert_eq!(row["issue_id"], child.id);
    assert_eq!(row["target"], "无匹配（兜底未开或无在线节点）");
    assert_eq!(row["match_detail"], "无在线候选节点");

    // 执行路径：失败行同样附明细。
    let out = super::project_resume(&store, &cluster, None, project.id, false, &actor)
        .await
        .unwrap();
    assert_eq!(out["dispatched"], 0);
    assert_eq!(out["failed"][0]["match_detail"], "无在线候选节点");

    // 节点上线（生产路径 = merge_real_node_info）后：匹配成功行不再附
    // 明细，target 解析为节点 id。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-rs".into(),
        name: "RsWorker".into(),
        address: "127.0.0.1:19999".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["rust".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    let out = super::project_resume(&store, &cluster, None, project.id, true, &actor)
        .await
        .unwrap();
    let row = &out["candidates"][0];
    assert_eq!(row["target"], "node-rs");
    assert!(
        row.get("match_detail").is_none(),
        "有匹配行不得附明细：{row}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P5/F2：dispatch_issue_core 冲突冻结闸（派发单一入口唯一落点）
// ---------------------------------------------------------------------------

/// 冻结闸先于集群依赖校验：冻结项目的单**无论**有没有集群、有没有目标，
/// 一律诚实拒绝且错误指向 project.resume——不因「集群未运行」混淆语义。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_dispatch_issue_core_rejects_frozen_project() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("dispatch-frozen-gate");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let pid = store
        .create_project("冻结闸项目", "", None, "", "", None)
        .unwrap()
        .id;
    let mut ni = nemesis_board::NewIssue {
        title: "冻结中的单".into(),
        ..Default::default()
    };
    ni.project_id = Some(pid);
    let issue = store.create_issue(ni).unwrap();

    // 冻结前：走到集群缺失报错（闸未拦）。
    let err =
        super::dispatch_issue_core(&store, None, issue.id, "node-b", &Actor::admin("t"), None)
            .unwrap_err();
    assert!(err.contains("集群未运行"), "闸未拦截时应报集群缺失：{err}");

    // 置冻结 → 拒绝且错误指明解冻路径。
    store.set_project_conflict_frozen(pid, true).unwrap();
    let err =
        super::dispatch_issue_core(&store, None, issue.id, "node-b", &Actor::admin("t"), None)
            .unwrap_err();
    assert!(err.contains("冲突冻结中"), "冻结闸必须拦：{err}");
    assert!(
        err.contains("project.resume"),
        "错误必须指向唯一解冻出口：{err}"
    );

    // 解冻 → 恢复正常校验链（回到集群缺失报错）。
    store.set_project_conflict_frozen(pid, false).unwrap();
    let err =
        super::dispatch_issue_core(&store, None, issue.id, "node-b", &Actor::admin("t"), None)
            .unwrap_err();
    assert!(err.contains("集群未运行"), "解冻后闸不再拦：{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

// BUG-②（2026-10-15 NB-1 事故）：挂有未确认计划的父单不可直接派发。
// ready（待确认）与 running（拆解进行中）都拦，错误指引确认或等 TTL；
// 过期计划不拦（TTL 惰性清理，派发闸自身 retain 同 plan.list 判据）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_dispatch_issue_core_rejects_issue_with_pending_plan() {
    // 本测试自己写 PLAN_CACHE/RUNNING_PLANS 进程级全局——与其他派发测试
    // 按 issue id 撞库（每 store 的 issue.id 都从 1 起），串行同纪律。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("dispatch-plan-guard");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "挂着计划的单".into(),
            ..Default::default()
        })
        .unwrap();

    // ready 计划在缓存 → 拦截且错误带 plan id + 确认指引（先于集群
    // 缺失报错——具体指引优先于泛化报错）。
    seed_plan("plan-guard-1", issue.id, vec![sub("子单A", vec![])]);
    let err =
        super::dispatch_issue_core(&store, None, issue.id, "node-b", &Actor::admin("t"), None)
            .unwrap_err();
    assert!(err.contains("未确认的拆解计划"), "{err}");
    assert!(err.contains("待确认"), "{err}");
    assert!(
        err.contains("plan-guard-1"),
        "错误应带 plan id 便于定位：{err}"
    );
    assert!(err.contains("确认"), "{err}");

    // 计划过期 → 不拦（走集群缺失报错）。601s 回拨需 Instant 纪元余量；
    // 刚启动系统回拨不了 → 环境边界诚实跳过（同过期态断言纪律）。
    let Some(expired_at) = Instant::now()
        .checked_sub(super::PLAN_TTL)
        .and_then(|t| t.checked_sub(std::time::Duration::from_secs(1)))
    else {
        eprintln!("SKIP 计划过期断言：系统刚启动，Instant 纪元余量不足 601s");
        return;
    };
    {
        let mut cache = super::PLAN_CACHE.lock();
        if let Some(p) = cache.get_mut("plan-guard-1") {
            p.created_at = expired_at;
        }
    }
    let err =
        super::dispatch_issue_core(&store, None, issue.id, "node-b", &Actor::admin("t"), None)
            .unwrap_err();
    assert!(
        err.contains("集群未运行"),
        "过期计划不拦（TTL 惰性清理同 plan.list）：{err}"
    );

    // running（拆解进行中）也拦。
    super::PLAN_CACHE.lock().remove("plan-guard-1");
    super::RUNNING_PLANS.lock().insert(
        "plan-guard-2".to_string(),
        super::RunningPlan {
            issue_id: issue.id,
            started_at: Instant::now(),
            attempt: 1,
            phase: "planning",
        },
    );
    let err =
        super::dispatch_issue_core(&store, None, issue.id, "node-b", &Actor::admin("t"), None)
            .unwrap_err();
    assert!(err.contains("拆解进行中"), "{err}");

    // 清场（进程级共享注册表，防污染其他用例）。
    super::RUNNING_PLANS.lock().remove("plan-guard-2");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 派发即指派回填（2026-09-17）：dispatch_issue_core 单一入口回填 assignee
// ---------------------------------------------------------------------------

// F1（2026-09-30 派发目标校验前置）：幽灵 target 在 API 层直接拒绝——
// 旧行为照单全收（dispatched=true）去 RPC 撞墙（真机行为分裂实证）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_dispatch_rejects_unknown_target_upfront() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("dispatch-ghost-target");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(
        nemesis_cluster::types::ClusterConfig::default(),
    ));
    // 注入空 client（2026-10-17 起 rpc client 前置检查先于 F1 归一化拒绝
    // ——裸 Cluster::new 会先撞「RPC client 未就绪」，走不到本测试要钉的
    // 「未知节点」臂；归一化拒绝发生在任何 send 之前，裸 client 足够）。
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "幽灵目标单".into(),
            ..Default::default()
        })
        .unwrap();
    let err = super::dispatch_issue_core(
        &store,
        Some(&cluster),
        issue.id,
        "Ghost-Node-XYZ",
        &Actor::admin("t"),
        None,
    )
    .unwrap_err();
    assert!(err.contains("未知节点"), "{err}");
    let after = store.get_issue(issue.id).unwrap();
    assert_eq!(after.status, IssueStatus::Backlog, "不得被推进 in_progress");
    assert_eq!(after.assignee, None, "不得回填指派");
    assert!(
        store.list_dispatches(issue.id).unwrap().is_empty(),
        "不得留下派发行"
    );
}

/// 派发回填 assignee=worker/目标节点；换目标重派时同点位改指。注入可解析
/// 到哑对端的 client（connect 即刻完成、ACK 永不到来——spawn 内 send 挂在
/// 外层超时上，同步断言窗口内回退路径不触发）：2026-10-17 rpc client 前置
/// 检查后，裸 `Cluster::new` 会在一切状态变更之前零副作用诚实拒绝——回填
/// 只在派发同步管线成功路径上观察（claim/状态转移/指派回填均在
/// submit_peer_chat 之后、fire-and-forget 发送之前落库）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_dispatch_backfills_assignee() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("dispatch-assignee-backfill");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(
        nemesis_cluster::types::ClusterConfig::default(),
    ));
    inject_silent_peer_client(
        &cluster,
        &[
            ("node-b", silent_peer_addr()),
            ("node-c", silent_peer_addr()),
        ],
    );
    // F1 派发目标校验前置（2026-09-30）：未知 target 在归一化处即被拒绝，
    // 本测试意图是回填点位（同步成功路径）——target 必须是注册表已知节点，
    // 且地址用哑对端（connect 即刻完成、ACK 永不到来，spawn 内 send 挂在
    // 外层超时上——同步断言窗口内回退路径不会触发）。
    cluster.register_node(nemesis_cluster::types::ExtendedNodeInfo {
        base: nemesis_cluster::types::NodeInfo {
            id: "node-b".into(),
            name: "Node-B".into(),
            role: nemesis_cluster::types::NodeRole::Worker,
            address: silent_peer_addr(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    cluster.register_node(nemesis_cluster::types::ExtendedNodeInfo {
        base: nemesis_cluster::types::NodeInfo {
            id: "node-c".into(),
            name: "Node-C".into(),
            role: nemesis_cluster::types::NodeRole::Worker,
            address: silent_peer_addr(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 未指派 → 派发给 node-b：同步管线全程成功（submit → 转移 → 回填 →
    // claim），连接失败留给 spawn 内异步终结，不影响同步断言。
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "回填指派的单".into(),
            ..Default::default()
        })
        .unwrap();
    super::dispatch_issue_core(
        &store,
        Some(&cluster),
        issue.id,
        "node-b",
        &Actor::admin("t"),
        None,
    )
    .unwrap();
    let after = store.get_issue(issue.id).unwrap();
    assert_eq!(
        after.assignee,
        Some(nemesis_board::assignment::AssignmentType::Worker)
    );
    assert_eq!(after.assignee_id.as_deref(), Some("node-b"));
    assert_eq!(
        after.status,
        nemesis_board::models::IssueStatus::InProgress,
        "回填发生在状态转移成功之后"
    );

    // 已显式指派 node-a → 派发 target=node-c：指派跟随实际派发目标改写
    //（D3 换节点重派同款语义——目标变了字段必须跟上）。
    let issue2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "改指的单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .assign_issue(
            issue2.id,
            Some(nemesis_board::assignment::AssignmentType::Worker),
            Some("node-a".into()),
            &Actor::admin("t"),
        )
        .unwrap();
    super::dispatch_issue_core(
        &store,
        Some(&cluster),
        issue2.id,
        "node-c",
        &Actor::admin("t"),
        None,
    )
    .unwrap();
    let after2 = store.get_issue(issue2.id).unwrap();
    assert_eq!(after2.assignee_id.as_deref(), Some("node-c"));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 看板项目档案 P6（F11）：project.progress 档案完整性投影
// ---------------------------------------------------------------------------

#[test]
fn test_project_progress_projects_archive_integrity_projection() {
    let dir = unique_dir("project-progress-integrity");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");

    // 未绑定档案目录 → archive_integrity=null（诚实缺省，前端隐藏徽章）。
    let bare = store
        .create_project("无档案项目", "", None, "", "", None)
        .unwrap();
    let out = super::project_progress(&store, bare.id).unwrap();
    assert!(
        out["archive_integrity"].is_null(),
        "未绑定目录应 null: {}",
        out["archive_integrity"]
    );
    assert_eq!(out["archive_missing_blocks"].as_array().unwrap().len(), 0);

    // 绑定目录 + manifest 带 missing_blocks → 投影浮出（单项目臂）。
    let archive = dir.join("archive-bound");
    nemesis_board::archive::ensure_scaffold(&archive, 0, "档案项目", "in_progress").unwrap();
    let mut manifest = nemesis_board::archive::read_manifest(&archive).unwrap();
    manifest.missing_blocks.push("NB-3:manifest".into());
    manifest.missing_blocks.push("NB-4:landed".into());
    nemesis_board::archive::write_manifest(&archive, &manifest).unwrap();
    let bound = store
        .create_project(
            "档案项目",
            "",
            None,
            "",
            "",
            Some(archive.to_str().unwrap()),
        )
        .unwrap();
    let out = super::project_progress(&store, bound.id).unwrap();
    assert_eq!(out["archive_integrity"], "ok");
    let missing = out["archive_missing_blocks"].as_array().unwrap();
    assert_eq!(missing.len(), 2, "D6 缺失块应浮出到进度投影: {missing:?}");
    assert_eq!(missing[0], "NB-3:manifest");

    // 全项目摘要臂同投影（列表页 ⚠ 徽章数据源）。
    let all = super::project_progress_all(&store).unwrap();
    let rows = all["projects"].as_array().unwrap();
    let bound_row = rows
        .iter()
        .find(|r| r["project_id"].as_i64() == Some(bound.id))
        .unwrap();
    assert_eq!(bound_row["archive_missing_blocks"][1], "NB-4:landed");
    let bare_row = rows
        .iter()
        .find(|r| r["project_id"].as_i64() == Some(bare.id))
        .unwrap();
    assert!(bare_row["archive_integrity"].is_null());

    // manifest 被手删 → 诚实缺省（不臆造 ok）。
    std::fs::remove_file(archive.join("project.json")).unwrap();
    let out = super::project_progress(&store, bound.id).unwrap();
    assert!(out["archive_integrity"].is_null());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// SAN-06/08 + EST（2026-09-16 横扫加固）：子树闭包联动 / 级联穿终态 / estop 闸内核
// ---------------------------------------------------------------------------

/// SAN-06：联动沿祖先链向上传播，且穿过终态中间层——done 父单 A 之下的
/// 取消叶子不再对上层 P 冻结：A 保持终态 + 缺口评论显形，P 推 in_review。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_sync_parent_status_propagates_up_through_terminal() {
    let dir = unique_dir("sync-upward");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let actor = nemesis_board::Actor::admin("test");

    let p = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "顶层".into(),
            ..Default::default()
        })
        .unwrap();
    let a = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "中间层".into(),
            parent_issue_id: Some(p.id),
            ..Default::default()
        })
        .unwrap();
    let b = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "叶子".into(),
            parent_issue_id: Some(a.id),
            ..Default::default()
        })
        .unwrap();

    // A 走到终态 done；P 置 in_progress（非终态）；B 取消。
    for st in [
        IssueStatus::Todo,
        IssueStatus::InProgress,
        IssueStatus::InReview,
        IssueStatus::Done,
    ] {
        store.transition_issue(a.id, st, &actor).unwrap();
    }
    store
        .transition_issue(p.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(b.id, IssueStatus::Todo, &actor)
        .unwrap();
    store
        .transition_issue(b.id, IssueStatus::Cancelled, &actor)
        .unwrap();

    super::sync_parent_status(&store, a.id, &actor).unwrap();

    // 终态中间层不被推列，但缺口评论显形（SAN-06 冻结缺口不再沉默）。
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Done);
    let a_gap = store
        .list_comments(a.id)
        .unwrap()
        .iter()
        .any(|cm| cm.content.contains("子任务存在未完成项") && cm.content.contains(&b.number));
    assert!(a_gap, "done 父单之下取消叶子必须显形");

    // 上层 P 被继续重估：in_review + 缺口评论。
    assert_eq!(store.get_issue(p.id).unwrap().status, IssueStatus::InReview);
    let p_gap = store
        .list_comments(p.id)
        .unwrap()
        .iter()
        .any(|cm| cm.content.contains("子任务存在未完成项") && cm.content.contains(&b.number));
    assert!(p_gap, "上层缺口评论必须列出深层叶子");

    let _ = std::fs::remove_dir_all(&dir);
}

/// SAN-08：级联遍历穿过终态中间节点——done 子单之下的孙子单和依赖该
/// done 单的下游单都必须被级联取消（旧手搓 BFS 在终态单上截断，死链
/// 从此不可见）；终态单本身不动。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_cascade_cancels_through_terminal_middle_node() {
    let dir = unique_dir("cascade-terminal");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let actor = nemesis_board::Actor::admin("test");

    let r = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "根".into(),
            ..Default::default()
        })
        .unwrap();
    let c = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "子(done中间层)".into(),
            parent_issue_id: Some(r.id),
            ..Default::default()
        })
        .unwrap();
    let g = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "孙(隐藏死链)".into(),
            parent_issue_id: Some(c.id),
            ..Default::default()
        })
        .unwrap();
    let x = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "依赖根".into(),
            ..Default::default()
        })
        .unwrap();
    let y = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "依赖done子单".into(),
            ..Default::default()
        })
        .unwrap();
    store.set_issue_dependencies(x.id, &[r.id]).unwrap();
    store.set_issue_dependencies(y.id, &[c.id]).unwrap();

    // C 走到 done（终态中间层）；R 取消（store 层直落，随后走联动）。
    for st in [
        IssueStatus::Todo,
        IssueStatus::InProgress,
        IssueStatus::InReview,
        IssueStatus::Done,
    ] {
        store.transition_issue(c.id, st, &actor).unwrap();
    }
    store
        .transition_issue(r.id, IssueStatus::Todo, &actor)
        .unwrap();
    store
        .transition_issue(r.id, IssueStatus::Cancelled, &actor)
        .unwrap();

    let cascaded = super::on_issue_settled(&store, None, r.id, &actor);

    // 终态单不动；其下/其依赖的下游全部取消。
    assert_eq!(store.get_issue(c.id).unwrap().status, IssueStatus::Done);
    for id in [g.id, x.id, y.id] {
        assert_eq!(
            store.get_issue(id).unwrap().status,
            IssueStatus::Cancelled,
            "issue {id} 必须被穿终态级联取消"
        );
    }
    for num in [&g.number, &x.number, &y.number] {
        assert!(cascaded.contains(num), "cascaded 缺 {num}: {cascaded:?}");
    }
    assert!(!cascaded.contains(&c.number), "终态单不得入级联列表");

    let _ = std::fs::remove_dir_all(&dir);
}

/// EST 闸判定可测内核：未装配/已释放 = 不冻结（等价旧行为），engaged =
/// 冻结；文案含释放指引。BOARD_ESTOP 全局槽的 engaged 态不在单测内联
/// 装配（OnceLock 进程级，与并行派发测试互踩）——接线由 gateway 装配点
/// 保证（BASELINE_PUSHER 同款先例）。
#[cfg(feature = "cluster")]
#[test]
fn test_estop_dispatch_frozen_pure_helper() {
    assert!(!super::estop_dispatch_frozen(None), "未装配 = 不冻结");
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    assert!(
        !super::estop_dispatch_frozen(Some(&estop)),
        "released = 不冻结"
    );
    estop.trigger();
    assert!(super::estop_dispatch_frozen(Some(&estop)), "engaged = 冻结");
    estop.release();
    assert!(!super::estop_dispatch_frozen(Some(&estop)), "释放后恢复");
    let err = super::estop_frozen_error();
    assert!(err.contains("estop") && err.contains("release"), "{err}");
}

// ===========================================================================
// AGT 覆盖率批次（2026-09-24）：board.rs miss 收口。
// 纯函数臂 / 钩子装配 / project_progress 环节矩阵 / config.set 全键 /
// audit 族 / autopilot cron 装配与同步。派发与 plan 链见附录二。
// ===========================================================================

/// AGT：可注入 cron/cluster/agent_loop 的 ctx 构造器（复制 make_ctx_with_service
/// 的 AppState 字面并参数化三个槽位；board service 必给）。
#[cfg(feature = "cluster")]
fn agt_make_ctx_ex(
    dir: &std::path::Path,
    service: nemesis_board::BoardService,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
    cron: Option<Arc<std::sync::Mutex<nemesis_cron::CronService>>>,
) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(dir.to_string_lossy().to_string()),
        home: Some(dir.to_string_lossy().to_string()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(agent_loop)),
        cluster,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron,
        board: Some(service),
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(dir.to_string_lossy().to_string()),
        home: Some(dir.to_string_lossy().to_string()),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// --- 附录一：纯函数与校验臂 ---

#[test]
fn agt_assignee_pairing_sanitize_and_filter_arms() {
    // (None, Some) 臂：assignee_id 单独出现 → 成对拒绝（59）
    let err = build_new_issue(
        &serde_json::json!({ "title": "t", "assignee_id": "w1" }),
        Actor::admin("s"),
    )
    .unwrap_err();
    assert!(err.contains("必须成对提供"), "{err}");

    // sanitize_filename：控制字符（77）/ 超长（80）/ 目录穿越取基本名
    assert!(sanitize_filename("bad\u{7}name").is_err());
    let long = "x".repeat(201);
    assert!(sanitize_filename(&long).is_err());
    assert_eq!(sanitize_filename("a/b\\..\\最终.md").unwrap(), "最终.md");
    assert!(sanitize_filename("..").is_err());

    // build_filter：未知 status（116）/ assignee 过滤（119）/ priority（125）
    let err = build_filter(&serde_json::json!({ "status": "不存在的状态" })).unwrap_err();
    assert!(err.contains("未知 status"), "{err}");
    let f = build_filter(&serde_json::json!({
        "assignee_type": "worker", "assignee_id": "w9", "priority": 2
    }))
    .unwrap();
    assert!(f.assignee.is_some());
    assert_eq!(f.priority, Some(2));
    // include 旗标臂：缺省排除（true）→ 显式放行（false）
    let f2 = build_filter(&serde_json::json!({})).unwrap();
    assert!(f2.exclude_archived_projects && f2.exclude_cancelled && f2.exclude_hidden);
    let f3 = build_filter(
        &serde_json::json!({ "include_archived_projects": true, "include_cancelled": true }),
    )
    .unwrap();
    assert!(!f3.exclude_archived_projects && !f3.exclude_cancelled);

    // build_new_issue：成对 assignee 落字段（178-179）+ origin（182-185）
    let ni = build_new_issue(
        &serde_json::json!({
            "title": "t", "priority": 1, "assignee_type": "manager_self", "assignee_id": "me",
            "origin_type": "claw", "origin_id": "origin-7"
        }),
        Actor::admin("s"),
    )
    .unwrap();
    assert_eq!(ni.assignee, Some(AssignmentType::ManagerSelf));
    assert_eq!(ni.assignee_id.as_deref(), Some("me"));
    assert_eq!(ni.priority, 1);
    let origin = ni.origin.expect("origin 应落字段");
    assert_eq!(origin.origin_type, "claw");
    assert_eq!(origin.origin_id, "origin-7");
}

#[cfg(feature = "cluster")]
#[test]
fn agt_dispatch_prompt_renders_all_optional_sections() {
    let dir = unique_dir("agt-prompt");
    let store = store_of(&make_ctx_with_board(&dir));
    // 2026-10-17 契约更新（NB-125 根修）：store 创建时自动生成 AC——要测
    // None 臂（兜底文案 231-232）必须显式抹掉生成值，模拟存量缺 AC 单。
    let mut issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "提示词装配单".into(),
            description: "  ".into(), // 空白背景 → 「（未提供）」臂（225）
            ..Default::default()
        })
        .unwrap();
    issue.acceptance_criteria = None;
    // 验收标准 None → 兜底文案（231-232）；assets/experience/feedback 全 Some（247/252/236-243）
    let p = build_dispatch_prompt(
        &issue,
        Some("## 任务资产\nASSET-SECTION"),
        Some("上轮差距：缺测试"),
        Some("## 团队经验\nEXP-SECTION"),
        None,
    );
    assert!(p.contains("（未提供）"), "{p}");
    assert!(p.contains("完成判据以背景描述为准"), "{p}");
    assert!(p.contains("上轮验收意见") && p.contains("缺测试"), "{p}");
    assert!(p.contains("ASSET-SECTION"), "{p}");
    assert!(p.contains("EXP-SECTION"), "{p}");
    assert!(p.contains("汇报格式"), "{p}");
    // 有验收标准 + 全 None → 走原文臂（231 Some 臂）且无附加段
    let issue2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "带验收".into(),
            acceptance_criteria: Some("必须包含回归测试".into()),
            ..Default::default()
        })
        .unwrap();
    let p2 = build_dispatch_prompt(&issue2, None, None, None, None);
    assert!(p2.contains("必须包含回归测试"), "{p2}");
    assert!(!p2.contains("上轮验收意见"), "{p2}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- PR5：重派轮次段 + 循环升级契约 ----------

#[cfg(feature = "cluster")]
#[test]
fn dispatch_prompt_renders_redispatch_round_and_loop_marker_contract() {
    let dir = std::env::temp_dir().join(format!("nmb-web-bd-pr5-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store =
        std::sync::Arc::new(nemesis_board::BoardStore::open(&dir.join("b.db"), "NB").unwrap());
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "提示词装配单PR5".into(),
            ..Default::default()
        })
        .unwrap();

    // 首派（attempt=None）：不渲染重派段（prompt 字节与历史一致）。
    let first = build_dispatch_prompt(&issue, None, None, None, None);
    assert!(!first.contains("重派轮次"), "{first}");
    assert!(!first.contains("REDISPATCH_LOOP"), "{first}");

    // 第 3 次派发（第 2 次重派）：渲染轮次段 + 循环升级契约。
    let third = build_dispatch_prompt(&issue, None, None, None, Some(3));
    assert!(third.contains("重派轮次（第 3 次派发）"), "{third}");
    assert!(third.contains("此前已被派发 2 次"), "{third}");
    assert!(third.contains("REDISPATCH_LOOP:"), "{third}");
    assert!(third.contains("升级通道"), "{third}");
    assert!(third.contains("已尝试方案清单"), "{third}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// project_review 钩子的进程级共享录制器：OnceLock 先装先赢，两个测试
/// （agt_hook_setters 与 test_notify_project_review_guards_and_hook_fire）
/// 都可能赢得装配权——闭包统一写这里，双方读同一真相，先后次序无关。
#[cfg(feature = "cluster")]
static AGT_PROJECT_REVIEW_FIRED: std::sync::LazyLock<std::sync::Mutex<Vec<i64>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

#[cfg(feature = "cluster")]
#[test]
fn agt_hook_setters_first_ok_second_rejected_and_notify_missing_issue() {
    let dir = unique_dir("agt-hooks");
    let store = store_of(&make_ctx_with_board(&dir));
    // 三个 OnceLock 钩子：本进程内首次装配成功（875-881/890-896/907-913），
    // 重复装配拒绝。project_review 与 notify 守卫用例共享录制器（赢家装
    // 配，双方读同一 Vec），避免 OnceLock 次序假失败。
    assert!(set_parent_review_hook(std::sync::Arc::new(|_pid| {})).is_ok());
    assert_eq!(
        set_parent_review_hook(std::sync::Arc::new(|_pid| {})).unwrap_err(),
        "parent review hook already set"
    );
    let _ = set_project_review_hook(std::sync::Arc::new(|pid| {
        AGT_PROJECT_REVIEW_FIRED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(pid);
    }));
    assert_eq!(
        set_project_review_hook(std::sync::Arc::new(|_pid| {})).unwrap_err(),
        "project review hook already set"
    );
    assert!(set_project_summary_hook(std::sync::Arc::new(|_pid| {})).is_ok());
    assert_eq!(
        set_project_summary_hook(std::sync::Arc::new(|_pid| {})).unwrap_err(),
        "project summary hook already set"
    );
    // 触发面对不存在 issue：get_issue Err → 诚实 no-op（946）
    notify_project_review_on_parent_done(&store, -99999);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- 附录一：project_progress 环节矩阵（1030-1065 全臂 + 1070）---

#[cfg(feature = "cluster")]
#[test]
fn agt_project_progress_stage_matrix_and_priority() {
    let dir = unique_dir("agt-progress");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let proj = store
        .create_project("环节矩阵", "", None, "", "", None)
        .unwrap();
    let mk = |title: &str| nemesis_board::NewIssue {
        title: title.to_string(),
        project_id: Some(proj.id),
        ..Default::default()
    };
    let done = store.create_issue(mk("已完成单")).unwrap();
    let review = store.create_issue(mk("验收单")).unwrap();
    let blocked = store.create_issue(mk("受阻单")).unwrap();
    let parked = store.create_issue(mk("停车单")).unwrap();
    let _backlog = store.create_issue(mk("待派单")).unwrap();
    let redo = store.create_issue(mk("重做单")).unwrap();
    let dispatched = store.create_issue(mk("执行单")).unwrap();
    let cancelled = store.create_issue(mk("取消单")).unwrap();
    let actor = Actor::admin("agt");
    // backlog → 任意非终态：一步到位
    store
        .transition_issue(done.id, IssueStatus::Done, &actor)
        .unwrap();
    store
        .transition_issue(review.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(review.id, IssueStatus::InReview, &actor)
        .unwrap();
    store
        .transition_issue(blocked.id, IssueStatus::Blocked, &actor)
        .unwrap();
    store
        .transition_issue(redo.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(dispatched.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(cancelled.id, IssueStatus::Cancelled, &actor)
        .unwrap();
    // 停车单：todo + ⏸ 系统评论（与 sweep 同源标记）
    store
        .transition_issue(parked.id, IssueStatus::Todo, &actor)
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: parked.id,
            author: Actor::system("board"),
            content: format!("⏸ {PARK_NOTICE_MARK}（无可用节点）"),
            parent_id: None,
            ctype: CommentType::System,
        })
        .unwrap();
    // 执行单：在途派发（claim）→ 执行中（区别于重做单的「无在途 in_progress」）
    assert!(
        store
            .try_claim_dispatch("agt-task-exec", dispatched.id, "node-x", &actor)
            .unwrap()
    );

    let full = super::project_progress(&store, proj.id).unwrap();
    let counts = &full["counts"];
    assert_eq!(counts["done"].as_i64(), Some(1));
    assert_eq!(counts["in_review"].as_i64(), Some(1));
    assert_eq!(counts["blocked"].as_i64(), Some(1));
    assert_eq!(counts["parked"].as_i64(), Some(1));
    assert_eq!(counts["backlog"].as_i64(), Some(1));
    assert_eq!(
        counts["in_progress"].as_i64(),
        Some(1),
        "无在途 in_progress=待重派"
    );
    assert_eq!(
        counts["dispatched"].as_i64(),
        Some(1),
        "有在途 in_progress=执行中"
    );
    assert_eq!(counts["cancelled"].as_i64(), Some(1));
    // 卡点优先级：受阻 > 其余
    assert_eq!(full["stage"].as_str(), Some("受阻"));
    // 逐单环节行完整
    assert_eq!(full["issues"].as_array().unwrap().len(), 8);

    // 各 stage 优先级单独验证：只建对应环节的单 → stage 命中对应臂
    let stage_of_only = |tag: &str, want: &str| {
        let p2 = store
            .create_project(&format!("只含{tag}"), "", None, "", "", None)
            .unwrap();
        let i = store
            .create_issue(nemesis_board::NewIssue {
                title: tag.to_string(),
                project_id: Some(p2.id),
                ..Default::default()
            })
            .unwrap();
        match tag {
            "review" => {
                store
                    .transition_issue(i.id, IssueStatus::InProgress, &actor)
                    .unwrap();
                store
                    .transition_issue(i.id, IssueStatus::InReview, &actor)
                    .unwrap();
            }
            "parked" => {
                store
                    .transition_issue(i.id, IssueStatus::Todo, &actor)
                    .unwrap();
                store
                    .add_comment(nemesis_board::NewComment {
                        issue_id: i.id,
                        author: Actor::system("board"),
                        content: PARK_NOTICE_MARK.to_string(),
                        parent_id: None,
                        ctype: CommentType::System,
                    })
                    .unwrap();
            }
            "redo" => {
                store
                    .transition_issue(i.id, IssueStatus::InProgress, &actor)
                    .unwrap();
            }
            "dispatched" => {
                store
                    .transition_issue(i.id, IssueStatus::InProgress, &actor)
                    .unwrap();
                store
                    .try_claim_dispatch(&format!("agt-task-{}", p2.id), i.id, "node-x", &actor)
                    .unwrap();
            }
            "done" => {
                store
                    .transition_issue(i.id, IssueStatus::Done, &actor)
                    .unwrap();
            }
            _ => {}
        }
        let out = super::project_progress(&store, p2.id).unwrap();
        assert_eq!(out["stage"].as_str(), Some(want), "project {want}");
    };
    stage_of_only("review", "验收中");
    stage_of_only("parked", "停车待恢复");
    stage_of_only("dispatched", "执行中");
    stage_of_only("redo", "待重派");
    stage_of_only("plain", "待派发");
    stage_of_only("done", "已完成");
    // 空项目 → 未拆解；project_id 不存在 → get_project Err 臂（1070）也走通
    let empty = store
        .create_project("空项目", "", None, "", "", None)
        .unwrap();
    assert_eq!(
        super::project_progress(&store, empty.id).unwrap()["stage"].as_str(),
        Some("未拆解")
    );
    assert_eq!(
        super::project_progress(&store, 424242).unwrap()["archive_integrity"],
        serde_json::json!(null),
        "get_project Err → (None, []) 诚实缺省"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- 附录一：config.get / config.set 全键（4259-4479）---

#[tokio::test]
async fn agt_board_config_get_set_all_keys_and_rejections() {
    let dir = unique_dir("agt-config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), "{}").unwrap();
    let ctx = make_ctx_with_board(&dir);

    // config.get：读出 board 段（默认值）
    let got = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert!(got.is_object());

    // 全键 round-trip：布尔 / 整数 / 字符串 / null
    for (key, value) in [
        ("auto_review", serde_json::json!(true)),
        ("auto_accept", serde_json::json!(true)),
        ("auto_close_parent", serde_json::json!(true)),
        ("unlimited_mode", serde_json::json!(false)),
        ("conflict_auto_resolve", serde_json::json!(true)),
        ("dispatch_fallback", serde_json::json!(true)),
        // 2026-09-29 三档化：字符串档位 round-trip（旧布尔上一行保真）。
        ("dispatch_fallback", serde_json::json!("off")),
        ("dispatch_fallback", serde_json::json!("role")),
        ("fingerprint_weighting", serde_json::json!(true)),
        ("worker_max_inflight", serde_json::json!(3)),
        ("dispatch_fallback_target", serde_json::json!("node-fix")),
        ("dispatch_fallback_target", serde_json::json!(null)),
        ("review.selfcheck", serde_json::json!(true)),
        ("review.auto_close_project", serde_json::json!(true)),
        ("plan.auto_confirm", serde_json::json!(true)),
        ("plan.model", serde_json::json!("big-model")),
        ("plan.model", serde_json::json!(null)),
        ("max_redispatch", serde_json::json!(7)),
        ("review.max_turns", serde_json::json!(4)),
        ("review.max_unsure_streak", serde_json::json!(11)),
        ("review.max_unsure_streak", serde_json::json!(0)),
        ("review.checkers", serde_json::json!(3)),
        ("budget.max_subissues_per_parent", serde_json::json!(9)),
        ("budget.max_total_redispatch", serde_json::json!(11)),
        ("budget.wall_clock_budget_secs", serde_json::json!(3600)),
        ("budget.max_tokens_per_parent", serde_json::json!(123456)),
        ("budget.max_send_retries", serde_json::json!(4)),
        ("dispatch_timeout_secs", serde_json::json!(1800)),
        ("discussion.retention_days", serde_json::json!(30)),
        (
            "discussion.max_agent_turns_per_thread",
            serde_json::json!(6),
        ),
        ("discussion.hourly_budget_per_node", serde_json::json!(20)),
        ("discussion.rate_limit_per_min", serde_json::json!(10)),
    ] {
        let out = dispatch(
            &ctx,
            "config.set",
            serde_json::json!({ "key": key, "value": value }),
        )
        .await
        .unwrap_or_else(|e| panic!("config.set {key} 失败: {e}"))
        .unwrap();
        assert_eq!(out["updated"], serde_json::json!(true), "{key}");
    }

    // 拒绝臂：类型错 / 范围错 / 未知键 / 缺 home
    for (key, value, why) in [
        ("worker_max_inflight", serde_json::json!(-1), "非负整数"),
        ("worker_max_inflight", serde_json::json!("x"), "非负整数"),
        (
            "dispatch_fallback_target",
            serde_json::json!(3),
            "字符串或 null",
        ),
        ("plan.model", serde_json::json!(true), "字符串或 null"),
        (
            "dispatch_fallback",
            serde_json::json!("wrong"),
            "\"off\"|\"role\"|\"full\"",
        ),
        ("max_redispatch", serde_json::json!(-2), "非负整数"),
        ("review.max_turns", serde_json::json!(1.5), "非负整数"),
        (
            "review.max_unsure_streak",
            serde_json::json!(-1),
            "非负整数",
        ),
        ("review.checkers", serde_json::json!(9), "1..=5"),
        ("budget.max_send_retries", serde_json::json!(-3), "非负整数"),
        (
            "budget.max_send_retries",
            serde_json::json!("many"),
            "非负整数",
        ),
        (
            "budget.wall_clock_budget_secs",
            serde_json::json!(null),
            "非负整数",
        ),
        (
            "dispatch_timeout_secs",
            serde_json::json!("soon"),
            "非负整数",
        ),
        ("discussion.retention_days", serde_json::json!(null), "整数"),
    ] {
        let err = dispatch(
            &ctx,
            "config.set",
            serde_json::json!({ "key": key, "value": value }),
        )
        .await
        .unwrap_err();
        assert!(err.contains(why), "{key} = {value}: {err}");
    }
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "not.a.key", "value": true }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("未知或不允许"), "{err}");

    // config.get 无 home → 报错（4260）
    let mut ctx_nohome = make_ctx_with_board(&dir);
    ctx_nohome.home = None;
    assert!(
        dispatch(&ctx_nohome, "config.get", serde_json::json!({}))
            .await
            .is_err()
    );
    // config.set 缺 value（4272）
    assert!(
        dispatch(
            &ctx,
            "config.set",
            serde_json::json!({ "key": "auto_review" })
        )
        .await
        .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- 附录一：audit 族（4208-4256）---

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_audit_list_offset_action_rollback_and_retry_merge() {
    let dir = unique_dir("agt-audit");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("agt");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "审计单".into(),
            ..Default::default()
        })
        .unwrap();
    // done 落定 → 造一条 auto_decide 决策 → rollback 有靶子
    store
        .transition_issue(issue.id, IssueStatus::Done, &actor)
        .unwrap();
    store
        .add_activity(
            issue.id,
            &actor,
            "auto_decide",
            Some("{\"verdict\":\"done\"}"),
        )
        .unwrap();
    let decisions = store
        .list_recent_activity_paged(50, 0, Some("auto_decide"))
        .unwrap();
    assert_eq!(decisions.len(), 1, "前置：恰一条 auto_decide");

    // audit.list：action 过滤 + offset 翻页（4217-4221）
    let out = dispatch(
        &ctx,
        "audit.list",
        serde_json::json!({ "limit": 5, "offset": 0, "action": "auto_decide" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["decisions"].as_array().unwrap().len(), 1);
    let out2 = dispatch(
        &ctx,
        "audit.list",
        serde_json::json!({ "offset": 1, "action": "auto_decide" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        out2["decisions"].as_array().unwrap().len(),
        0,
        "offset 跳过"
    );

    // audit.rollback：activity_id 定位 auto_decide + done 单 → 回 in_review（4224-4237）
    let act_id = decisions[0].activity.id;
    let out3 = dispatch(
        &ctx,
        "audit.rollback",
        serde_json::json!({ "activity_id": act_id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out3["rolled_back"], serde_json::json!(true));
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
    // 缺 activity_id（4229）
    assert!(
        dispatch(&ctx, "audit.rollback", serde_json::json!({}))
            .await
            .is_err()
    );

    // audit.retry_merge：钩子未安装 → 诚实报错（4247-4249）
    let err = dispatch(
        &ctx,
        "audit.retry_merge",
        serde_json::json!({ "id": issue.id }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("钩子未安装"), "{err}");
    // 安装钩子（OnceLock 进程级；本批次唯一安装点）→ 结果透传（4250-4256）
    set_retry_merge_hook(std::sync::Arc::new(|i| {
        Ok(serde_json::json!({ "retried": i.id, "tasks": [] }))
    }));
    let out4 = dispatch(
        &ctx,
        "audit.retry_merge",
        serde_json::json!({ "id": issue.id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out4["retried"], serde_json::json!(issue.id));
    let _ = std::fs::remove_dir_all(&dir);
}

// --- 附录一：M5-1 评审重触发（review.retrigger，2026-10-10 机制加固）---

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_review_retrigger_gate_hook_and_trace() {
    let dir = unique_dir("agt-retrig");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("agt");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "重触发单".into(),
            ..Default::default()
        })
        .unwrap();
    // backlog → in_progress → in_review（合法阶梯）。
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(issue.id, IssueStatus::InReview, &actor)
        .unwrap();

    // ① 非 in_review 诚实拒绝（钩子装不装都不该走到）。
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &actor)
        .unwrap();
    let err = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "id": issue.id }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("in_review"), "{err}");
    store
        .transition_issue(issue.id, IssueStatus::InReview, &actor)
        .unwrap();

    // ② 缺 number/id → 参数错误。
    assert!(
        dispatch(&ctx, "review.retrigger", serde_json::json!({}))
            .await
            .is_err()
    );

    // ③ 钩子未安装 → 诚实报错（本测试是 REVIEW_RETRIGGER_HOOK 唯一安装
    //    点，安装前 get() 恒 None；number 寻址顺带覆盖）。
    let err = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "number": issue.number }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("钩子未安装"), "{err}");

    // ④ 安装钩子 → number 寻址命中、按 issue.id 触发、留痕评论、响应形态。
    let fired = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let fired2 = fired.clone();
    let _ = set_review_retrigger_hook(std::sync::Arc::new(move |issue_id: i64| {
        fired2.lock().unwrap().push(issue_id);
    }));
    let out = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "number": issue.number }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["retriggered"], serde_json::json!(true));
    assert_eq!(out["issue"], serde_json::json!(issue.number));
    assert_eq!(
        fired.lock().unwrap().as_slice(),
        &[issue.id],
        "钩子按 issue.id 触发"
    );
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("人工触发重新验收评审")),
        "必须留系统评论：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );

    // ⑤ number 不存在 → 诚实报错（不 panic 不静默）。
    let err = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "number": "NB-99999" }),
    )
    .await
    .unwrap_err();
    assert!(!err.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// --- 项目收口复评臂（2026-10-17 NB-101 project_escalate_human 死路根修）---

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_project_retrigger_arm_gates_hook_and_trace() {
    let dir = unique_dir("agt-proj-retrig");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("agt");
    let pid = store
        .create_project("复评项目", "", None, "", "", None)
        .unwrap()
        .id;
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "项目父单".into(),
            project_id: Some(pid),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(parent.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(parent.id, IssueStatus::InReview, &actor)
        .unwrap();
    store
        .transition_issue(parent.id, IssueStatus::Done, &actor)
        .unwrap();

    // ① 项目不存在 → 诚实报错。
    let err = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "project_id": 987654 }),
    )
    .await
    .unwrap_err();
    assert!(!err.is_empty(), "{err}");

    // ② 钩子未安装 → 诚实报错（PROJECT_REVIEW_RETRIGGER_HOOK 唯一安装点
    //    在本测试；装之前的 get() 恒 None）。
    let err = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "project_id": pid }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("钩子未安装"), "{err}");

    // ③ 安装钩子 → project_id 寻址命中、按 project_id 触发、父单留痕。
    let fired = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let fired2 = fired.clone();
    let _ = set_project_review_retrigger_hook(std::sync::Arc::new(move |project_id: i64| {
        fired2.lock().unwrap().push(project_id);
    }));
    let out = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "project_id": pid }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["retriggered"], serde_json::json!(true));
    assert_eq!(out["project_id"], serde_json::json!(pid));
    assert_eq!(
        fired.lock().unwrap().as_slice(),
        &[pid],
        "钩子按 project_id 触发"
    );
    let comments = store.list_comments(parent.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("人工触发项目收口复评")),
        "父单必须留系统注记：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );

    // ④ 非 done 顶层父单 → 诚实拒绝（先收口父单再复评项目）。
    let pid2 = store
        .create_project("复评项目2", "", None, "", "", None)
        .unwrap()
        .id;
    let _parent2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "未完成父单".into(),
            project_id: Some(pid2),
            ..Default::default()
        })
        .unwrap();
    let err = dispatch(
        &ctx,
        "review.retrigger",
        serde_json::json!({ "project_id": pid2 }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("非 done"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- 附录一：autopilot cron 装配 / 同步 / 摘除（3279-3390）---

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_autopilot_cron_arm_patch_follow_sync_and_disarm() {
    let dir = unique_dir("agt-apcron");
    let cron = std::sync::Arc::new(std::sync::Mutex::new(nemesis_cron::CronService::new(
        &dir.join("cron.db").to_string_lossy(),
    )));
    let svc_store = BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(Arc::new(svc_store), NodeRole::Coordinator),
        None,
        None,
        Some(cron.clone()),
    );
    let store = store_of(&ctx);

    // autopilot.create：cron 注入 → 即时登记并回填 job id（arm_autopilot_job 3270-3274）
    let created = dispatch(
        &ctx,
        "autopilot.create",
        serde_json::json!({
            "name": "同步规则", "title": "定时活", "cron": "*/5 * * * *",
            "enabled": true, "description": "d"
        }),
    )
    .await;
    let created = match created {
        Ok(v) => v.unwrap(),
        Err(e) => panic!("autopilot.create 失败: {e}"),
    };
    let ap_id = created["autopilot"]["id"].as_i64().unwrap();
    let job_id = created["autopilot"]["cron_job_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!job_id.is_empty(), "已登记 job id");

    // autopilot.update：cron 变更 → patch 跟随既有 job（patch 臂 3294-3305）
    let updated = dispatch(
        &ctx,
        "autopilot.update",
        serde_json::json!({ "id": ap_id, "cron": "*/7 * * * *", "enabled": false }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        updated["autopilot"]["cron_job_id"].as_str(),
        Some(job_id.as_str()),
        "跟随 patch 不换 job"
    );

    // sync_autopilot_jobs：孤儿清理 + 缺失补登记 + 既有跟随（3349-3390）
    {
        let svc = cron.lock().unwrap();
        let sched = nemesis_cron::CronSchedule {
            kind: "cron".into(),
            at_ms: None,
            every_ms: None,
            expr: Some("*/9 * * * *".into()),
            tz: None,
        };
        // BUG-20261024-10 后 `board-ap:{i64}` 是保留命名空间：内部旁路
        // （add_job_internal）是这类名字的唯一来源，这里用它造幽灵作业。
        // 孤儿判据按全形态——整数 id 且未注册才在射程内。
        svc.add_job_internal(
            "board-ap:999999",
            sched,
            "",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    }
    // 造缺失：另一条规则不登记（cron_job_id 留空）
    let missing = store
        .create_autopilot(&nemesis_board::NewAutopilot {
            name: "缺登记".into(),
            title: "t".into(),
            cron: "*/11 * * * *".into(),
            description: String::new(),
            priority: nemesis_board::models::priority::MEDIUM,
            project_id: None,
            target: String::new(),
            enabled: true,
            auto_plan: false,
            acceptance_criteria: None,
        })
        .unwrap();
    // job 名约定 = board-ap:{autopilot_id}（不是规则名）
    let missing_job_name = format!("board-ap:{}", missing.id);
    let rearmed = super::sync_autopilot_jobs(&cron, &store).unwrap();
    assert!(rearmed >= 1, "至少补登记缺失规则: {rearmed}");
    {
        let svc = cron.lock().unwrap();
        let names: Vec<String> = svc.list_jobs(true).iter().map(|j| j.name.clone()).collect();
        assert!(
            !names.iter().any(|n| n == "board-ap:999999"),
            "孤儿 job 已清理: {names:?}"
        );
        assert!(names.contains(&missing_job_name), "{names:?}");
    }
    // disarm：job 不存在 → warn 不炸（3328-3330）
    let ap = store.get_autopilot(ap_id).unwrap();
    let mut ghost = ap.clone();
    ghost.cron_job_id = Some("board-ap:no-such-job".to_string());
    super::disarm_autopilot_job(&ctx, &ghost);
    // cron 未注入的 ctx → arm no-op Ok（3270-3272）
    let bare = make_ctx_with_board(&dir);
    let _ = dispatch(
        &bare,
        "autopilot.create",
        serde_json::json!({
            "name": "无cron规则", "title": "t", "cron": "*/13 * * * *"
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn agt_autopilot_validate_and_issues_listing() {
    let dir = unique_dir("agt-aplist");
    let ctx = make_ctx_with_board(&dir);
    // 非法 cron 表达式 → create 拒绝（validate_schedule 臂）
    let r = dispatch(
        &ctx,
        "autopilot.create",
        serde_json::json!({ "name": "坏规则", "title": "t", "cron": "不是cron" }),
    )
    .await;
    assert!(r.is_err(), "非法 cron 必须被拒");
    // autopilot.issues：按 origin 列单（3765-3771）
    let out = dispatch(
        &ctx,
        "autopilot.runs",
        serde_json::json!({ "id": 1, "limit": 5 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issues"], serde_json::json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// AGT 覆盖率批次（2026-09-24）附录二：派发闸 / project.resume / sweep /
// 重平衡 / 级联取消 / planner / plan 链（全部 cluster-gated）。
// ===========================================================================

#[cfg(feature = "cluster")]
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
#[cfg(feature = "cluster")]
use nemesis_agent::types::AgentConfig;

/// 恒输出非 JSON 文本的 provider（planner 解析三轮全败 / 链失败臂）。
#[cfg(feature = "cluster")]
struct AgtBadPlannerProvider;
/// 恒输出合法 plan JSON 的 provider。
#[cfg(feature = "cluster")]
struct AgtGoodPlannerProvider;
/// 恒 Err 的 provider（planner LLM 调用失败臂）。
#[cfg(feature = "cluster")]
struct AgtDownProvider;

#[cfg(feature = "cluster")]
const AGT_PLAN_JSON: &str = r#"[{"title":"子任务甲","acceptance_criteria":"存在回归测试"},{"title":"子任务乙","depends_on":[0]}]"#;

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl LlmProvider for AgtBadPlannerProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "我建议拆成两步：先写代码再补测试。".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl LlmProvider for AgtGoodPlannerProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: AGT_PLAN_JSON.to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl LlmProvider for AgtDownProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("provider down".to_string())
    }
}

/// 捕获全部 user 消息文本的 provider（D2 等待预算注入断言；恒回合法计划）。
#[cfg(feature = "cluster")]
struct AgtPromptCaptureProvider(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl LlmProvider for AgtPromptCaptureProvider {
    async fn chat(
        &self,
        _: &str,
        messages: Vec<LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        if let Some(m) = messages.iter().rfind(|m| m.role == "user") {
            self.0.lock().unwrap().push(m.content.clone());
        }
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: AGT_PLAN_JSON.to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// D2 等待预算注入（run_planner 新参）：Some(secs) → user prompt 带
/// 「讨论评审等待预算」段与具体值；None → 段不渲染（字节与历史一致）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_run_planner_discussion_wait_hint_injected() {
    let dir = unique_dir("agt-wait-hint");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "预算注入父单".into(),
            ..Default::default()
        })
        .unwrap();

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtPromptCaptureProvider(seen.clone())),
        AgentConfig::default(),
    ));
    super::run_planner(&al, &parent, Vec::new(), None, Some(600), None)
        .await
        .unwrap();
    let first = seen.lock().unwrap()[0].clone();
    assert!(first.contains("讨论评审等待预算"), "{first}");
    assert!(first.contains("review_wait_secs = 600"), "{first}");
    assert!(first.contains("0=不等直接裁决"), "{first}");

    // None = 不渲染（无该段——传导面关断时不污染既有 prompt 字节）。
    super::run_planner(&al, &parent, Vec::new(), None, None, None)
        .await
        .unwrap();
    let second = seen.lock().unwrap()[1].clone();
    assert!(!second.contains("讨论评审等待预算"), "{second}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_run_planner_surrender_llm_fail_and_success() {
    let dir = unique_dir("agt-planner");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "拆解父单".into(),
            ..Default::default()
        })
        .unwrap();

    // 解析失败重试 ≤2 → 3 轮后诚实放弃（重试循环全臂）
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtBadPlannerProvider),
        AgentConfig::default(),
    ));
    let err = super::run_planner(&al, &parent, Vec::new(), None, None, None)
        .await
        .unwrap_err();
    assert!(err.contains("连续 3 轮无法解析"), "{err}");

    // LLM 调用失败：不消耗解析轮，独立归类
    let al_down = Arc::new(AgentLoop::new(
        Box::new(AgtDownProvider),
        AgentConfig::default(),
    ));
    let err2 = super::run_planner(&al_down, &parent, Vec::new(), None, None, None)
        .await
        .unwrap_err();
    assert!(err2.contains("planner LLM 调用失败"), "{err2}");

    // 合法 JSON → 直接成功返回（Ok 臂）
    let al_ok = Arc::new(AgentLoop::new(
        Box::new(AgtGoodPlannerProvider),
        AgentConfig::default(),
    ));
    let (subs, downgrades) = super::run_planner(
        &al_ok,
        &parent,
        vec!["过往经验: 先补测试".into()],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[1].depends_on, vec![0]);
    assert!(downgrades.is_empty(), "严格可过的计划零降级");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_execute_plan_chain_planned_failure_and_auto_confirm() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-chain");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "链父单".into(),
            ..Default::default()
        })
        .unwrap();
    let actor = Actor::admin("agt");
    let hub = EventHub::new();
    let al_bad = Arc::new(AgentLoop::new(
        Box::new(AgtBadPlannerProvider),
        AgentConfig::default(),
    ));

    // planner 失败 → 链内发布 plan_failed + Err 透传
    let err = super::execute_plan_chain(
        &store,
        None,
        al_bad.clone(),
        parent.clone(),
        actor.clone(),
        "agt-plan-fail",
        None,
        Some(&hub),
        1,
    )
    .await
    .unwrap_err();
    assert!(err.contains("连续 3 轮无法解析"), "{err}");

    // 成功拆解 + auto_confirm 关（home None → fail-closed 回人工）→ planned
    let al_ok = Arc::new(AgentLoop::new(
        Box::new(AgtGoodPlannerProvider),
        AgentConfig::default(),
    ));
    let out = super::execute_plan_chain(
        &store,
        None,
        al_ok.clone(),
        parent.clone(),
        actor.clone(),
        "agt-plan-ok",
        None,
        Some(&hub),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out["status"], serde_json::json!("planned"));
    assert_eq!(out["subs"], serde_json::json!(2));
    // 预览已入缓存 → 二段确认可消费（confirm_plan 走建单 + 依赖边）
    let out2 = dispatch(
        &ctx,
        "issue.plan",
        serde_json::json!({ "id": parent.id, "confirm": true, "plan_id": "agt-plan-ok" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out2["created"].as_array().unwrap().len(), 2, "子单落库");
    // 依赖边落库：乙依赖甲
    let children = store.list_children(parent.id).unwrap();
    let with_dep = children.iter().find(|c| c.title == "子任务乙").unwrap();
    assert!(
        !store.dependencies_of(with_dep.id).unwrap().is_empty(),
        "depends_on → 依赖边"
    );

    // auto_confirm 开（home 下 config.json）→ 直接发车 + 系统评论 + 决策审计
    let home = unique_dir("agt-chain-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        r#"{"board":{"plan":{"auto_confirm":true}}}"#,
    )
    .unwrap();
    let parent2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "自动发车父单".into(),
            ..Default::default()
        })
        .unwrap();
    let out3 = super::execute_plan_chain(
        &store,
        None,
        al_ok,
        parent2.clone(),
        actor,
        "agt-plan-auto",
        Some(&home),
        Some(&hub),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out3["status"], serde_json::json!("dispatched"), "{out3}");
    assert_eq!(
        store.list_children(parent2.id).unwrap().len(),
        2,
        "自动发车已建子单"
    );
    let comments = store.list_comments(parent2.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("auto_confirm 已开启")),
        "系统评论留痕: {:?}",
        comments
    );
    // 决策审计：auto_decide 已入活动流
    let acts = store.list_activity(parent2.id).unwrap();
    assert!(
        acts.iter().any(|a| a.action == "auto_decide"),
        "auto_decide 审计: {acts:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// 按提示词分流的 provider：起草形态（user prompt 含「起草」）回可配置
/// 的 AC 文本；planner 形态回合法计划 JSON。`draft_reply: None` = 起草
/// 形态也回 JSON 数组（护栏拒绝臂——模型误入拆解角色）。
#[cfg(feature = "cluster")]
struct AgtAcDraftProvider {
    draft_reply: Option<&'static str>,
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl LlmProvider for AgtAcDraftProvider {
    async fn chat(
        &self,
        _: &str,
        messages: Vec<LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let drafting = messages
            .iter()
            .rfind(|m| m.role == "user")
            .map(|m| m.content.contains("起草"))
            .unwrap_or(false);
        let content = if drafting {
            self.draft_reply
                .unwrap_or(r#"[{"title":"误入拆解形态"}]"#)
                .to_string()
        } else {
            AGT_PLAN_JSON.to_string()
        };
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content,
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 预防层（NB-101 AC 缺失根修）：父单 AC 为空 → 拆解链自动起草落库 +
/// 系统评论留痕；已有基线不动（用户显式 AC 最大）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_execute_plan_chain_drafts_parent_ac_when_empty() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-ac-draft");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("agt");
    let hub = EventHub::new();
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtAcDraftProvider {
            draft_reply: Some("- 游戏可打开\n- 无脚本错误"),
        }),
        AgentConfig::default(),
    ));

    // 空基线父单：起草落库 + 评论留痕。2026-10-17 契约更新：store 建单
    // 自动生成 AC——LLM 起草闸只在 AC 真空时触发（存量单形态），本地抹掉
    // 生成值保持该路径覆盖。
    let mut parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "无基线父单".into(),
            ..Default::default()
        })
        .unwrap();
    parent.acceptance_criteria = None;
    let out = super::execute_plan_chain(
        &store,
        None,
        al.clone(),
        parent.clone(),
        actor.clone(),
        "agt-ac-draft-1",
        None,
        Some(&hub),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out["status"], serde_json::json!("planned"), "{out}");
    let reloaded = store.get_issue(parent.id).unwrap();
    assert_eq!(
        reloaded.acceptance_criteria.as_deref(),
        Some("- 游戏可打开\n- 无脚本错误"),
        "父单 AC 已起草落库"
    );
    let comments = store.list_comments(parent.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("已依据子单计划自动起草")),
        "起草评论留痕: {comments:?}"
    );

    // 已有基线父单：不动（起草只补空，不覆盖用户显式 AC）
    let parent2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "有基线父单".into(),
            acceptance_criteria: Some("用户基线".into()),
            ..Default::default()
        })
        .unwrap();
    super::execute_plan_chain(
        &store,
        None,
        al,
        parent2.clone(),
        actor,
        "agt-ac-draft-2",
        None,
        Some(&hub),
        1,
    )
    .await
    .unwrap();
    let reloaded2 = store.get_issue(parent2.id).unwrap();
    assert_eq!(
        reloaded2.acceptance_criteria.as_deref(),
        Some("用户基线"),
        "已有基线不被起草覆盖"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 预防层失败臂：起草产物为 JSON 形态（护栏拒绝）/ LLM 失败都不阻塞
/// 发车——链照常 planned，父单 AC 保持空，失败评论指路 AI 起草。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_execute_plan_chain_ac_draft_failure_does_not_block() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-ac-draft-fail");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    // 2026-10-17 契约更新：本地抹掉 store 生成值，保持「AC 真空 → 起草失败
    // 不落坏草稿」路径覆盖。
    let mut parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "起草失败父单".into(),
            ..Default::default()
        })
        .unwrap();
    parent.acceptance_criteria = None;
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtAcDraftProvider { draft_reply: None }),
        AgentConfig::default(),
    ));
    let out = super::execute_plan_chain(
        &store,
        None,
        al,
        parent.clone(),
        Actor::admin("agt"),
        "agt-ac-draft-fail",
        None,
        Some(&EventHub::new()),
        1,
    )
    .await
    .unwrap();
    assert_eq!(
        out["status"],
        serde_json::json!("planned"),
        "起草失败不阻塞: {out}"
    );
    // 2026-10-17 契约更新：起草失败不落坏草稿——store 基线（建单自动生成
    // AC）逐字节不变，没有被失败产物污染。
    let ac_baseline = store
        .get_issue(parent.id)
        .unwrap()
        .acceptance_criteria
        .clone();
    assert!(
        ac_baseline.is_some(),
        "建单自动生成 AC 已在场（新契约基线）"
    );
    assert_eq!(
        store.get_issue(parent.id).unwrap().acceptance_criteria,
        ac_baseline,
        "护栏拒绝后父单 AC 基线不变（不落库坏草稿）"
    );
    let comments = store.list_comments(parent.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("自动起草失败") && c.content.contains("AI 起草")),
        "失败评论指路: {comments:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// WSAPI `issue.draft_ac`：草稿回前端、不落库；父单 prompt 汇总子单材料。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_issue_draft_ac_returns_draft_without_persisting() {
    let dir = unique_dir("agt-draft-ac-ws");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtAcDraftProvider {
            draft_reply: Some("- 草稿条目一\n- 草稿条目二"),
        }),
        AgentConfig::default(),
    ));
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        None,
        Some(al),
        None,
    );
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "待起草父单".into(),
            ..Default::default()
        })
        .unwrap();
    let child = store
        .create_issue(nemesis_board::NewIssue {
            title: "已交付子单".into(),
            parent_issue_id: Some(parent.id),
            acceptance_criteria: Some("子基线".into()),
            ..Default::default()
        })
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: child.id,
            author: Actor::agent("w"),
            content: "交付完成".to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    // 草稿不落库（保存决定权在用户）。2026-10-17 契约更新：store 建单已
    // 自动生成 AC——「不落库」的正确断言 = 起草前后 AC 逐字节不变。
    let ac_before = store.get_issue(parent.id).unwrap().acceptance_criteria;
    assert!(ac_before.is_some(), "建单自动生成 AC 已在场（新契约基线）");
    let out = dispatch(
        &ctx,
        "issue.draft_ac",
        serde_json::json!({ "id": parent.id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue"], serde_json::json!(parent.number), "{out}");
    assert_eq!(
        out["draft"],
        serde_json::json!("- 草稿条目一\n- 草稿条目二")
    );
    assert_eq!(
        store.get_issue(parent.id).unwrap().acceptance_criteria,
        ac_before,
        "起草只回前端预填，不改 store"
    );

    // agent 未运行 → 诚实报错（不是 panic / 静默空）。
    let dir2 = unique_dir("agt-draft-ac-noagent");
    let ctx2 = make_ctx_with_board(&dir2);
    let err = dispatch(&ctx2, "issue.draft_ac", serde_json::json!({ "id": 1 }))
        .await
        .unwrap_err();
    assert!(err.contains("agent 未运行"), "{err}");

    // commands() 注册（L1 契约：system.commands 汇总发布）。
    let cmds = super::BoardHandler.commands();
    assert!(
        cmds.contains(&"issue.draft_ac"),
        "issue.draft_ac 未注册: {cmds:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_issue_plan_stage_one_spawns_chain_when_agent_ready() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-plan1");
    let store = BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtGoodPlannerProvider),
        AgentConfig::default(),
    ));
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(Arc::new(store), NodeRole::Coordinator),
        None,
        Some(al),
        None,
    );
    let issue = store_of(&ctx)
        .create_issue(nemesis_board::NewIssue {
            title: "一段拆解单".into(),
            ..Default::default()
        })
        .unwrap();
    let out = dispatch(&ctx, "issue.plan", serde_json::json!({ "id": issue.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], serde_json::json!("planning"));
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    assert!(plan_id.starts_with("plan-"), "{out}");
    // 后台链以 GoodPlanner 收尾（预览入缓存）；轮询等它就绪后 confirm 走
    // 通二段（一段 spawn 全臂）。缓存未就绪时 confirm 报「不存在」且不
    // 消费——安全重试。
    let mut out2 = None;
    for _ in 0..50 {
        match dispatch(
            &ctx,
            "issue.plan",
            serde_json::json!({ "id": issue.id, "confirm": true, "plan_id": plan_id }),
        )
        .await
        {
            Ok(Some(v)) => {
                out2 = Some(v);
                break;
            }
            Ok(None) => break,
            Err(e) if e.contains("plan_id 不存在") => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("unexpected confirm error: {e}"),
        }
    }
    let out2 = out2.expect("后台 plan 链应在轮询窗口内就绪");
    assert_eq!(out2["created"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_dispatch_gates_matrix() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-dispatch");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(
        nemesis_cluster::types::ClusterConfig::default(),
    ));
    // 注入可解析到哑对端的 client（2026-10-17 rpc client 前置检查先于各
    // 状态闸后的派发管线——裸 Cluster::new 会零副作用早拒，走不到注册表
    // 归一化/拓扑闸/完整派发管线；哑 listener 让 send 挂在外层超时，同步
    // 断言窗口内 claim 行保持在途）。
    inject_silent_peer_client(&cluster, &[("node-b", silent_peer_addr())]);
    cluster.register_node(nemesis_cluster::types::ExtendedNodeInfo {
        base: nemesis_cluster::types::NodeInfo {
            id: "node-b".into(),
            name: "Node-B".into(),
            role: nemesis_cluster::types::NodeRole::Worker,
            address: silent_peer_addr(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    // 拓扑硬闸臂的 far-node-9 同理须是已知节点（F1 归一化拒绝先于拓扑闸）。
    cluster.register_node(nemesis_cluster::types::ExtendedNodeInfo {
        base: nemesis_cluster::types::NodeInfo {
            id: "far-node-9".into(),
            name: "Far-Node-9".into(),
            role: nemesis_cluster::types::NodeRole::Worker,
            address: "10.0.0.9:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        Some(cluster),
        None,
        None,
    );
    let actor = Actor::admin("agt");

    // 状态闸：终态不可派发
    let done_issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "已完成".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(done_issue.id, IssueStatus::Done, &actor)
        .unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": done_issue.id, "target": "node-b" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("不可派发"), "{err}");

    // 重复派发闸：已有在途派发
    let busy = store
        .create_issue(nemesis_board::NewIssue {
            title: "在途单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .try_claim_dispatch("agt-task-busy", busy.id, "node-b", &actor)
        .unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": busy.id, "target": "node-b" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("已有进行中的派发"), "{err}");

    // 显式 target 空白 → 拒
    let blank = store
        .create_issue(nemesis_board::NewIssue {
            title: "空白目标".into(),
            ..Default::default()
        })
        .unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": blank.id, "target": "   " }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("target 不能为空"), "{err}");

    // manager_self 指派 → 本机执行不支持远端
    let mine = store
        .create_issue(nemesis_board::NewIssue {
            title: "本机单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .assign_issue(
            mine.id,
            Some(AssignmentType::ManagerSelf),
            Some("me".into()),
            &actor,
        )
        .unwrap();
    let err = dispatch(&ctx, "issue.dispatch", serde_json::json!({ "id": mine.id }))
        .await
        .unwrap_err();
    assert!(err.contains("manager_self"), "{err}");

    // 无指派无 target → 缺少派发目标
    let orphan = store
        .create_issue(nemesis_board::NewIssue {
            title: "无目标单".into(),
            ..Default::default()
        })
        .unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": orphan.id }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("缺少派发目标"), "{err}");

    // worker 指派 → target 解析自 assignee_id → 走完整派发管线到同步成功
    //（submit → backlog → in_progress 派发转移 → 回填 → claim；连接失败在
    // spawn 内异步终结，不进同步断言窗口）。
    let resolved = store
        .create_issue(nemesis_board::NewIssue {
            title: "指派解析单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .assign_issue(
            resolved.id,
            Some(AssignmentType::Worker),
            Some("node-b".into()),
            &actor,
        )
        .unwrap();
    dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": resolved.id }),
    )
    .await
    .unwrap();
    assert_eq!(
        store.get_issue(resolved.id).unwrap().status,
        IssueStatus::InProgress,
        "backlog → in_progress 派发转移放行臂"
    );

    // 已在途单再派 → 重复闸先行（状态闸已放行 in_progress）
    let err2 = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": resolved.id }),
    )
    .await
    .unwrap_err();
    assert!(err2.contains("已有进行中的派发"), "{err2}");

    // 冲突冻结闸
    let proj = store
        .create_project("冻结项目", "", None, "", "", None)
        .unwrap();
    let frozen = store
        .create_issue(nemesis_board::NewIssue {
            title: "冻结单".into(),
            project_id: Some(proj.id),
            ..Default::default()
        })
        .unwrap();
    store.set_project_conflict_frozen(proj.id, true).unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": frozen.id, "target": "node-b" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("冲突冻结"), "{err}");
    store.set_project_conflict_frozen(proj.id, false).unwrap();

    // 拓扑硬闸：远端 target + file: 锚点 = 拒
    let anchored = store
        .create_issue(nemesis_board::NewIssue {
            title: "锚点单".into(),
            acceptance_criteria: Some("[CHECK] file:src/main.rs exists".into()),
            ..Default::default()
        })
        .unwrap();
    let err = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": anchored.id, "target": "far-node-9" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("拒绝派发") && err.contains("file:"), "{err}");

    // project_id 指向已删除项目（存量幽灵行——引用完整性闸 2026-10-17 后
    // create 不再接受，直连 SQLite 删行复刻存量形态）：两个诚实降级点，
    // 命中哪个取决于 BASELINE_PUSHER OnceLock 的装配赢家（基线测试同进程
    // 并行）——基线装配先跑 = push_dispatch_baseline 的 get_project Err
    // 弃派（「派发中止」）；未装配先跑 = 联动 link_project_on_dispatch 只
    // warn，派发照走（注入 client 后同步成功）。两臂都是诚实路径。
    let ghost_project = store
        .create_project("矩阵幽灵项目", "", None, "", "", None)
        .unwrap();
    let ghost_proj = store
        .create_issue(nemesis_board::NewIssue {
            title: "幽灵项目单".into(),
            project_id: Some(ghost_project.id),
            ..Default::default()
        })
        .unwrap();
    {
        let conn = rusqlite::Connection::open(dir.join("board.db")).unwrap();
        conn.execute("DELETE FROM project WHERE id = ?1", [ghost_project.id])
            .unwrap();
    }
    // 基线 pusher 未装配：联动只 warn，派发照走完整同步管线。
    if let Err(e) = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": ghost_proj.id, "target": "node-b" }),
    )
    .await
    {
        assert!(e.contains("派发中止") || e.contains("not found"), "{e}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_project_resume_frozen_replay_refreeze_and_candidate_filter() {
    let dir = unique_dir("agt-resume");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let cluster = offline_cluster(&dir, "node-a");
    let actor = Actor::admin("agt");
    let proj = store
        .create_project("恢复项目", "", None, "", "", None)
        .unwrap();
    let mk = |title: &str| nemesis_board::NewIssue {
        title: title.to_string(),
        project_id: Some(proj.id),
        ..Default::default()
    };
    // 候选过滤臂：in_review 跳过 / 有子单的父单跳过 / 在途跳过；候选 =
    // 两个干净叶子（父单本身因有子单被跳过，其子叶子是合法派发单元）。
    let review = store.create_issue(mk("验收中")).unwrap();
    store
        .transition_issue(review.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(review.id, IssueStatus::InReview, &actor)
        .unwrap();
    let parent = store.create_issue(mk("父单")).unwrap();
    let child = store
        .create_issue(nemesis_board::NewIssue {
            title: "叶子".into(),
            project_id: Some(proj.id),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let busy = store.create_issue(mk("在途")).unwrap();
    store
        .try_claim_dispatch("agt-task-busy2", busy.id, "node-b", &actor)
        .unwrap();
    let leaf = store.create_issue(mk("干净叶子")).unwrap();

    // 回放钩子（OnceLock 进程级唯一安装点）：switch=false → 保留冻结；
    // switch=true → 解冻并落回放（覆盖 resume 冻结三分支）。
    let unfreeze = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook_store = store.clone();
    let hook_flag = unfreeze.clone();
    super::set_resume_replay_hook(Arc::new(move |pid| {
        if hook_flag.load(std::sync::atomic::Ordering::SeqCst) {
            hook_store.set_project_conflict_frozen(pid, false).unwrap();
        }
        Ok(serde_json::json!({ "unfrozen": true, "manual_commit": 1 }))
    }));

    // ① 未冻结 + dry_run → 候选预览（三类非候选全过滤；无目标行附
    //    match_detail 差什么明细）
    let out = super::project_resume(&store, &cluster, None, proj.id, true, &actor)
        .await
        .unwrap();
    assert_eq!(out["dry_run"], serde_json::json!(true), "{out}");
    let cands = out["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 2, "{out:?}");
    let ids: Vec<i64> = cands
        .iter()
        .map(|c| c["issue_id"].as_i64().unwrap())
        .collect();
    assert!(ids.contains(&leaf.id) && ids.contains(&child.id), "{ids:?}");
    assert!(
        !ids.contains(&review.id) && !ids.contains(&busy.id) && !ids.contains(&parent.id),
        "非候选被过滤: {ids:?}"
    );
    for c in cands {
        assert!(
            c["target"].as_str().unwrap().contains("无匹配"),
            "离线无目标: {c}"
        );
        assert!(c["match_detail"].is_string(), "{c}");
    }

    // ② 冻结 + dry_run → 预览不落副作用（冻结先行段 dry_run 臂）
    store.set_project_conflict_frozen(proj.id, true).unwrap();
    let out2 = super::project_resume(&store, &cluster, None, proj.id, true, &actor)
        .await
        .unwrap();
    assert_eq!(out2["frozen"], serde_json::json!(true), "{out2}");
    assert!(out2.get("candidates").is_none(), "冻结预览无候选: {out2}");

    // ③ 冻结 + 执行 + 钩子保留冻结 → 补合并再冲突 → 重新冻结回人工
    let out3 = super::project_resume(&store, &cluster, None, proj.id, false, &actor)
        .await
        .unwrap();
    assert_eq!(out3["resumed"], serde_json::json!(false), "{out3}");
    assert_eq!(out3["frozen"], serde_json::json!(true));
    assert_eq!(
        out3["replay"]["unfrozen"],
        serde_json::json!(true),
        "钩子回放摘要随冻结响应披露"
    );

    // ④ 钩子解冻 → 走完整恢复：候选 1、离线无目标 → failed 行；
    // conflict_replay 披露。
    unfreeze.store(true, std::sync::atomic::Ordering::SeqCst);
    let out4 = super::project_resume(&store, &cluster, None, proj.id, false, &actor)
        .await
        .unwrap();
    assert_eq!(out4["dry_run"], serde_json::json!(false));
    assert_eq!(out4["dispatched"], serde_json::json!(0));
    assert_eq!(out4["total_candidates"], serde_json::json!(2));
    assert_eq!(out4["failed"].as_array().unwrap().len(), 2, "{out4}");
    assert!(
        out4["conflict_replay"].is_object(),
        "回放摘要随响应披露: {out4}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_sweep_and_rebalance_offline_arms() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-sweep");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let cluster = offline_cluster(&dir, "node-a");
    let actor = Actor::admin("agt");
    // planner 来源叶子（停车场粗筛命中：origin_type=planner）
    let parked = store
        .create_issue(nemesis_board::NewIssue {
            title: "planner 叶子".into(),
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".into(),
                origin_id: "NB-1".into(),
            }),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store.list_dispatch_park_candidates().unwrap(),
        vec![parked.id]
    );

    // sweep：候选 1、无在线节点派不出去（两种 notify 形态都走）
    let (n, d, _f) =
        super::sweep_parked_dispatches_with_config(None, &store, &cluster, &actor, true);
    assert_eq!((n, d), (1, 0), "候选 1、离线派发 0");
    let (n2, d2, _) =
        super::sweep_parked_dispatches_with_config(None, &store, &cluster, &actor, false);
    assert_eq!((n2, d2), (1, 0));

    // 重平衡早退臂：
    let mut cfg = nemesis_config::BoardFlagConfig::default();
    // ① cap=0（不限）→ 无意义 → 0
    cfg.worker_max_inflight = 0;
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-a", &actor).await,
        0
    );
    // ② 目标已满载（node-a 在途 1 ≥ cap 1）→ 0
    cfg.worker_max_inflight = 1;
    store
        .try_claim_dispatch("agt-task-a1", parked.id, "node-a", &actor)
        .unwrap();
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-a", &actor).await,
        0
    );
    // ③ 排队队列为空（排除目标自身后）→ 0
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-a", &actor).await,
        0
    );
    // ④ 有排队单（node-b 在途）但目标不在在线候选 → 0
    let moved = store
        .create_issue(nemesis_board::NewIssue {
            title: "被偷单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .try_claim_dispatch("agt-task-b1", moved.id, "node-b", &actor)
        .unwrap();
    cfg.worker_max_inflight = 2;
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-zzz", &actor).await,
        0,
        "目标无候选（不在线）"
    );
    // ⑤ 目标在线候选存在但 RPC client 缺失（离线集群）→ 0
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-a", &actor).await,
        0,
        "RPC client 缺失臂"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn agt_issue_cancel_with_dispatch_cascades_union_edges() {
    let dir = unique_dir("agt-cancel");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let cluster = offline_cluster(&dir, "node-a");
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        Some(cluster),
        None,
        None,
    );
    let actor = Actor::admin("agt");

    // root（有在途派发）→ child（父子边 + 在途派发）；done2（终态跳过）；
    // dep（依赖边）
    let root = store
        .create_issue(nemesis_board::NewIssue {
            title: "取消根".into(),
            ..Default::default()
        })
        .unwrap();
    let child = store
        .create_issue(nemesis_board::NewIssue {
            title: "连带子单".into(),
            parent_issue_id: Some(root.id),
            ..Default::default()
        })
        .unwrap();
    let done2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "已完不连带".into(),
            parent_issue_id: Some(root.id),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(done2.id, IssueStatus::Done, &actor)
        .unwrap();
    let dep = store
        .create_issue(nemesis_board::NewIssue {
            title: "依赖死链".into(),
            ..Default::default()
        })
        .unwrap();
    store.set_issue_dependencies(dep.id, &[root.id]).unwrap();
    // 在途派发：root 与 child 各一
    store
        .try_claim_dispatch("agt-task-root", root.id, "node-b", &actor)
        .unwrap();
    store
        .try_claim_dispatch("agt-task-child", child.id, "node-b", &actor)
        .unwrap();

    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": root.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], serde_json::json!(true));
    assert!(
        out["task_id"].is_string(),
        "有在途派发的取消披露 task_id: {out}"
    );
    // 级联清单：父子边（child）+ 依赖边（dep）；done2 终态不连带
    let cascaded = out["cascade_cancelled"].as_array().unwrap();
    assert_eq!(cascaded.len(), 2, "{out}");
    assert_eq!(
        store.get_issue(child.id).unwrap().status,
        IssueStatus::Cancelled
    );
    assert_eq!(
        store.get_issue(dep.id).unwrap().status,
        IssueStatus::Cancelled
    );
    assert_eq!(store.get_issue(done2.id).unwrap().status, IssueStatus::Done);
    // 在途派发连带取消
    assert!(store.get_active_dispatch(child.id).unwrap().is_none());
    assert!(store.get_active_dispatch(root.id).unwrap().is_none());
    // 级联留痕评论
    let comments = store.list_comments(child.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("级联取消")),
        "{comments:?}"
    );
    // 重复取消：已 cancelled → 幂等（不走转移，不再级联出新单）
    let out2 = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": root.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out2["cancelled"], serde_json::json!(true));
    assert_eq!(
        out2["cascade_cancelled"].as_array().unwrap().len(),
        0,
        "全终态后无新连带"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// AGT 覆盖率批次（2026-09-24）附录三：基线下发（BASELINE_PUSHER 通路）。
// push_dispatch_baseline 五Outcome × 传输四形态；OnceLock 进程级唯一安装，
// transport 行为由 AtomicU8 现场切换（安装一次，段内变轨）。
// block_in_place 要求 multi_thread runtime。
// ===========================================================================

/// 传输 mock：mode 0=dedup（对端已有同档） 1=no handler 错误 2=普通错误
/// 3=over_limit 状态回复。
#[cfg(feature = "cluster")]
struct AgtPushTransport {
    mode: Arc<std::sync::atomic::AtomicU8>,
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl nemesis_cluster::outbox::TransferTransport for AgtPushTransport {
    async fn call(
        &self,
        _peer: &str,
        _action: &str,
        _payload: serde_json::Value,
        _timeout: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        match self.mode.load(std::sync::atomic::Ordering::SeqCst) {
            1 => Err("no handler for transfer.begin".to_string()),
            2 => Err("boom".to_string()),
            3 => Ok(serde_json::json!({ "status": "over_limit" })),
            _ => Ok(serde_json::json!({ "status": "dedup" })),
        }
    }
}

#[cfg(feature = "cluster")]
#[tokio::test(flavor = "multi_thread")]
async fn agt_push_dispatch_baseline_paths() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("agt-baseline");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let actor = Actor::admin("agt");

    // 未冻结项目 + 带文件的目录（init commit 收进树 → staging 非空）
    let proj_dir = dir.join("repo");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(proj_dir.join("README.md"), "baseline v1\n").unwrap();
    let proj = store
        .create_project(
            "基线项目",
            "",
            None,
            "",
            "",
            Some(proj_dir.to_str().unwrap()),
        )
        .unwrap();
    let mk = |title: &str, pid: Option<i64>| nemesis_board::NewIssue {
        title: title.to_string(),
        project_id: pid,
        ..Default::default()
    };
    let with_proj = store.create_issue(mk("带项目", Some(proj.id))).unwrap();
    let no_proj = store.create_issue(mk("无项目", None)).unwrap();
    // 「幽灵项目」引用（存量态）：引用完整性闸（2026-10-17）后 create 已
    // 不接受不存在的 project_id——先建真实项目再直连 SQLite 删行复刻存量
    // 数据形态（push_dispatch_baseline 的 get_project Err 防御臂只对存量
    // 幽灵行有意义）。
    let ghost_proj = store
        .create_project("将被删的项目", "", None, "", "", None)
        .unwrap();
    let ghost_issue = store
        .create_issue(mk("幽灵项目", Some(ghost_proj.id)))
        .unwrap();
    {
        let conn = rusqlite::Connection::open(dir.join("board.db")).unwrap();
        conn.execute("DELETE FROM project WHERE id = ?1", [ghost_proj.id])
            .unwrap();
    }

    // ① pusher 未安装（OnceLock 先装先赢——本测试必须在安装前覆盖此臂）
    let out = super::push_dispatch_baseline(&store, &with_proj, "agt-baseline-0", "node-b");
    assert_eq!(out.unwrap(), None, "未安装 pusher = 直通");

    // 安装（mode 0=dedup；护栏 0=不限，AtomicU64 现场切换）
    let mode = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let limit = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let installed = super::install_baseline_pusher(Arc::new(super::BaselinePusher {
        transport: Arc::new(AgtPushTransport { mode: mode.clone() }),
        source_node_id: "node-a".to_string(),
        max_bytes: Box::new({
            let limit = limit.clone();
            move || limit.load(std::sync::atomic::Ordering::SeqCst)
        }),
    }));
    assert!(installed, "本测试是进程内唯一安装点");

    // ② 无 project_id → 直通
    let out = super::push_dispatch_baseline(&store, &no_proj, "agt-baseline-1", "node-b");
    assert_eq!(out.unwrap(), None);

    // ③ project_id 指向已删除项目（存量幽灵行）→ get_project Err 中止派发
    let out = super::push_dispatch_baseline(&store, &ghost_issue, "agt-baseline-2", "node-b");
    assert!(out.is_err(), "幽灵项目必须中止: {out:?}");

    // ④ 项目无 directory → 直通
    let plain_proj = store
        .create_project("无目录项目", "", None, "", "", None)
        .unwrap();
    let plain_issue = store
        .create_issue(mk("无目录", Some(plain_proj.id)))
        .unwrap();
    let out = super::push_dispatch_baseline(&store, &plain_issue, "agt-baseline-3", "node-b");
    assert_eq!(out.unwrap(), None);

    // ⑤ dedup → Delivered → 基线落账（40 位 HEAD commit）
    let out = super::push_dispatch_baseline(&store, &with_proj, "agt-baseline-5", "node-b");
    let commit = out.unwrap().expect("dedup 应返回基线 commit");
    assert_eq!(commit.len(), 40, "{commit}");

    // ⑥ 本地护栏（max_bytes=1 < 文件字节）→ 不出网直接 Overlimit
    limit.store(1, std::sync::atomic::Ordering::SeqCst);
    let out = super::push_dispatch_baseline(&store, &with_proj, "agt-baseline-6", "node-b");
    let err = out.unwrap_err();
    assert!(err.contains("超护栏"), "{err}");
    limit.store(0, std::sync::atomic::Ordering::SeqCst);

    // ⑦ 对端 over_limit 状态 → 诚实弃派
    mode.store(3, std::sync::atomic::Ordering::SeqCst);
    let out = super::push_dispatch_baseline(&store, &with_proj, "agt-baseline-7", "node-b");
    assert!(out.unwrap_err().contains("超护栏"));

    // ⑧ 传输 Err 含 no handler → 版本升级提示
    mode.store(1, std::sync::atomic::Ordering::SeqCst);
    let out = super::push_dispatch_baseline(&store, &with_proj, "agt-baseline-8", "node-b");
    let err = out.unwrap_err();
    assert!(
        err.contains("no handler") && err.contains("版本过旧"),
        "{err}"
    );

    // ⑨ 传输普通 Err → 原样透传（无升级提示）
    mode.store(2, std::sync::atomic::Ordering::SeqCst);
    let out = super::push_dispatch_baseline(&store, &with_proj, "agt-baseline-9", "node-b");
    let err = out.unwrap_err();
    assert!(err.contains("boom") && !err.contains("版本过旧"), "{err}");

    // ⑩ 完整派发管线带基线：dedup 模式恢复 → 基线成功后照走同步管线
    //    （submit → claim + 状态推进；连接失败在 spawn 内异步终结）。
    let cluster = offline_cluster(&dir, "node-a");
    // F1 派发目标校验前置：node-b 须在注册表（否则归一化处被拒）。RPC
    // client 前置检查（2026-10-17）要求注入可用 client 才能走到完整管线。
    cluster.register_node(nemesis_cluster::types::ExtendedNodeInfo {
        base: nemesis_cluster::types::NodeInfo {
            id: "node-b".into(),
            name: "Node-B".into(),
            role: nemesis_cluster::types::NodeRole::Worker,
            // 哑对端 listener：同步断言窗口内 connect/ACK 都不终结（不可
            // 路由地址的 connect 失败快慢随主机路由环境漂移，竞态不可接受）。
            address: silent_peer_addr(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    mode.store(0, std::sync::atomic::Ordering::SeqCst);
    // 哑 client 会派生「peer unresolvable」瞬时 Err → spawn 内回滚与同步
    // 断言竞态——本测试是 multi_thread，必须给可解析到哑对端的 client。
    inject_silent_peer_client(&cluster, &[("node-b", silent_peer_addr())]);
    super::dispatch_issue_core(&store, Some(&cluster), with_proj.id, "node-b", &actor, None)
        .unwrap();
    assert!(
        store.get_active_dispatch(with_proj.id).unwrap().is_some(),
        "基线成功后 claim 已落定"
    );
    assert_eq!(
        store.get_issue(with_proj.id).unwrap().status,
        IssueStatus::InProgress
    );

    // ⑪ 已是 in_progress 的单再派 → 状态推进 else 臂（跳过转移）+ 基线
    //    重推幂等（dedup 覆盖同 task 行）。
    let second = store.create_issue(mk("第二条", Some(proj.id))).unwrap();
    store
        .transition_issue(second.id, IssueStatus::InProgress, &actor)
        .unwrap();
    super::dispatch_issue_core(&store, Some(&cluster), second.id, "node-b", &actor, None).unwrap();
    assert_eq!(
        store.get_issue(second.id).unwrap().status,
        IssueStatus::InProgress,
        "已是 in_progress 不重复转移"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// W5 覆盖率补批（2026-09-25）：board.rs 末批 miss 收口。
// 纯函数臂 / 派发正路径（离线 cluster + 注入 RPC client）/ 重平衡搬运环 /
// sweep 失败计数 / spawned plan 链 / handler 杂项臂。失败注入臂（基线下发
// Err、claim 竞态、状态转移回滚、estop 全局 engaged 装配）保持不测，与
// 既有纪律一致。
// ===========================================================================

/// 给离线 cluster 注入空 RPC client（resolver:None → spawn 体内 dispatch()
/// 瞬时「peer unresolvable」Err → 回滚）。**只用于 current_thread 测试**：
/// spawn 体在断言窗口内不被调度（运行时关闭时丢弃未执行任务），同步断言
/// 看到的都是 pre-rollback 状态；multi_thread 测试必须改用
/// `inject_silent_peer_client`（回滚会并发跑，哑 client 会竞态）。
#[cfg(feature = "cluster")]
fn w5_inject_rpc(cluster: &nemesis_cluster::cluster::Cluster) {
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
}

/// 让一个 worker 节点上线（生产路径 = merge_real_node_info；tags/caps
/// 可空，供渲染「—」/空 caps 分支）。
#[cfg(feature = "cluster")]
fn w5_online_peer(
    cluster: &nemesis_cluster::cluster::Cluster,
    id: &str,
    tags: &[&str],
    caps: &[&str],
) {
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: id.to_string(),
        name: format!("{id}-name"),
        address: "127.0.0.1:19999".to_string(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".to_string(),
        capabilities: caps.iter().map(|s| (*s).to_string()).collect(),
        tags: tags.iter().map(|s| (*s).to_string()).collect(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".to_string(),
        build: String::new(),
    });
}

/// BOARD_ESTOP 槽装配（467-469）：未 engaged 态装配 = 行为中立（纯判定
/// 函数已钉 released 不冻结）；重复装配被 OnceLock 诚实拒绝。附带钉
/// build_dispatch_prompt 的非空描述段（225）。
#[cfg(feature = "cluster")]
#[test]
fn w5_estop_slot_install_and_prompt_description_arm() {
    // 槽可能被先跑的并行测试占掉（OnceLock）——两次装配至多一次 true，
    // 重复装配必须被诚实拒绝。
    let first = super::install_board_estop(Arc::new(nemesis_agent::estop::EstopState::new()));
    let second = super::install_board_estop(Arc::new(nemesis_agent::estop::EstopState::new()));
    assert!(
        !(first && second),
        "OnceLock 装配：第二次必须失败（两者皆未 engaged，行为中立）"
    );

    // 派发 prompt：非空 description 走「## 描述」正文段（225）。
    let mut issue = bare_issue(None, &[]);
    issue.description = "先跑回归再改实现".to_string();
    let prompt = super::build_dispatch_prompt(
        &issue,
        None,
        Some("评审意见：补边界用例"),
        Some("经验：先写测试"),
        None,
    );
    assert!(
        prompt.contains("## 背景") && prompt.contains("先跑回归再改实现"),
        "{prompt}"
    );
    assert!(
        prompt.contains("上轮验收意见") && prompt.contains("评审意见：补边界用例"),
        "{prompt}"
    );
    assert!(prompt.contains("经验：先写测试"), "{prompt}");
}

/// run_planner 重试轮带集群画像（1355-1361）：解析连续失败 3 轮，第 2/3
/// 轮 prompt 追加 cluster_profile 段——放弃文案与无画像路径一致。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_run_planner_retry_appends_cluster_profile() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-planner-profile");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "画像父单".into(),
            ..Default::default()
        })
        .unwrap();
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtBadPlannerProvider),
        AgentConfig::default(),
    ));
    let err = super::run_planner(
        &al,
        &parent,
        Vec::new(),
        Some("- PyWorker [role=worker tags=python]".to_string()),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("连续 3 轮无法解析"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// confirm_plan（1665-1742）：空标题子单（绕过 parse_plan 直构）→ 落库
/// `?` 诚实失败（1693）；合法子单 + 匹配 peer 在线 → 派发波 Ok(Some)
/// 计入 dispatched（1718），依赖未满足的子单 deferred、父单 in_progress。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_confirm_plan_empty_title_err_and_dispatch_wave() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-confirm");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "确认父单".into(),
            ..Default::default()
        })
        .unwrap();

    let bad = nemesis_board::PlannedSubIssue {
        title: "   ".into(),
        description: String::new(),
        required_role: String::new(),
        required_tags: Vec::new(),
        required_profession: String::new(),
        required_backend: String::new(),
        required_capabilities: vec![],
        acceptance_criteria: String::new(),
        depends_on: Vec::new(),
    };
    let err = super::confirm_plan(&store, None, &parent, vec![bad], &actor, &[]).unwrap_err();
    assert!(err.contains("title must not be empty"), "{err}");

    let cluster = offline_cluster(&dir, "w5-coord");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-py", &["python"], &[]);
    let out = super::confirm_plan(
        &store,
        Some(&cluster),
        &parent,
        vec![sub("子甲", vec![]), sub("子乙", vec![0])],
        &actor,
        &[],
    )
    .unwrap();
    assert_eq!(out["dispatched"], serde_json::json!(1), "{out}");
    assert_eq!(out["deferred"].as_array().unwrap().len(), 1);
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::InProgress,
        "首派 → 父单联动 in_progress"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 闸臂集：在途派发重复派发暂缓（1847-1849）；兄弟有在途 → 父单不转
/// 受阻（2043-2049）；无子单 sync 幂等（2538-2540）；级联取消根缺失早退
/// （2736-2739）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_gates_active_dispatch_stalled_parent_childless_cascade() {
    let dir = unique_dir("w5-gates");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "闸父单".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "子A", vec![]);
    let b = planner_child(&store, parent.id, "子B", vec![]);

    // 已有在途派发 → Ok(None)（1847-1849）。
    store
        .insert_dispatch("w5-t-a", a.id, "node-x", &actor)
        .unwrap();
    assert_eq!(
        super::dispatch_subissue_auto_with_config(None, &store, None, a.id, &actor, false).unwrap(),
        None
    );

    // 兄弟有在途 → all_unstarted=false → 父单保持 backlog（2047-2049）。
    super::mark_parent_blocked_if_stalled(&store, &b, &actor);
    assert_eq!(
        store.get_issue(parent.id).unwrap().status,
        IssueStatus::Backlog,
        "链上还有在途单，父单不得转受阻"
    );

    // 无子单 sync：Ok 幂等（子树空早退）。
    super::sync_parent_status(&store, b.id, &actor).unwrap();

    // 级联取消：根单不存在 → 空列表早退。
    assert!(super::cascade_cancel_dependents(&store, None, 999_999, &actor).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 匹配器臂（2317-2325 / 2480-2481）：词表外角色（qa）转标签语义参与
/// 匹配——qa 标签节点上线即命中；空 tags 候选的失败明细显示「—」。
#[cfg(feature = "cluster")]
#[test]
fn w5_rank_out_of_vocab_role_and_empty_tags_detail() {
    let dir = unique_dir("w5-rank");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let cluster = offline_cluster(&dir, "w5-rank-c");

    // 无候选：角色归一照常返回空。
    assert!(
        super::rank_dispatch_candidates(&store, &cluster, &bare_issue(Some("qa"), &[])).is_empty()
    );

    // qa 标签节点上线 → 转标签后命中。
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-qa", &["qa"], &[]);
    let ranked = super::rank_dispatch_candidates(&store, &cluster, &bare_issue(Some("qa"), &[]));
    assert_eq!(ranked, vec!["node-qa".to_string()], "{ranked:?}");

    // 明细：候选无 tags → 段位显示「—」。
    let detail = super::match_failure_detail(
        &bare_issue(Some("qa"), &[]),
        &[cand("node-empty", "worker", &[])],
    );
    assert!(detail.contains("—"), "{detail}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 自动派发内核（2914-2943）：assignee=Worker 但 assignee_id 缺失 →
/// false（2918-2920）；正常指派 + peer 在线 + RPC client → 派出返回
/// true（2921-2927）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_auto_dispatch_without_id_false_then_ok_true() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-autodisp");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let cluster = offline_cluster(&dir, "w5-ad-c");
    let cfg = nemesis_config::BoardFlagConfig {
        auto_dispatch: true,
        ..Default::default()
    };

    // assignee_id 缺失（store 层拒绝该形态，直构 issue 绕过）。
    let mut ghost = bare_issue(None, &[]);
    ghost.assignee = Some(AssignmentType::Worker);
    assert!(!super::auto_dispatch_with_config(
        Some(&cfg),
        &store,
        Some(&cluster),
        &ghost,
        &actor
    ));

    // 正常：worker 指派 + peer 在线 → 派出。
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-py", &[], &[]);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "自动派发单".into(),
            assignee: Some(AssignmentType::Worker),
            assignee_id: Some("node-py".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(super::auto_dispatch_with_config(
        Some(&cfg),
        &store,
        Some(&cluster),
        &issue,
        &actor
    ));
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// autopilot（2991-2995 / 3053-3060）：target 非空模板预指派 worker；
/// 触发时集群缺 RPC client → 建单后派发失败诚实 map_err 透出（3058-3059）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_autopilot_target_assignee_and_fire_dispatch_fail() {
    let dir = unique_dir("w5-ap-target");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let ap = store
        .create_autopilot(&nemesis_board::NewAutopilot {
            name: "带目标".to_string(),
            cron: "0 9 * * *".to_string(),
            title: "周期任务 {date}".to_string(),
            description: "w5".to_string(),
            priority: priority::MEDIUM,
            project_id: None,
            target: "node-ghost".to_string(),
            enabled: true,
            auto_plan: false,
            acceptance_criteria: None,
        })
        .unwrap();

    let ni = super::autopilot_new_issue(&ap, &actor);
    assert_eq!(ni.assignee, Some(AssignmentType::Worker));
    assert_eq!(ni.assignee_id.as_deref(), Some("node-ghost"));

    // 集群在（无 RPC client、无 peer）→ 派发核心失败 → map_err 文案。
    let cluster = offline_cluster(&dir, "w5-ap-c");
    let err = super::fire_autopilot(&store, Some(&cluster), &ap, &actor, None).unwrap_err();
    assert!(err.contains("已创建但派发失败"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// disarm（3324-3336）：job_id 指向幽灵 job → remove_job false WARN 臂；
/// cron 锁被毒化 → poisoned WARN 臂。摘除是 best-effort，两臂都不 panic。
#[cfg(feature = "cluster")]
#[test]
fn w5_disarm_ghost_job_and_poisoned_lock() {
    let dir = unique_dir("w5-disarm");
    let cron = Arc::new(std::sync::Mutex::new(nemesis_cron::CronService::new(
        &dir.join("cron.db").to_string_lossy(),
    )));
    let svc_store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(svc_store.clone(), NodeRole::Coordinator),
        None,
        None,
        Some(cron.clone()),
    );
    let store = store_of(&ctx);

    // 幽灵 job：登记 id 在 cron 里不存在 → remove_job false。
    let mut ap = p3_ap(&store, "w5-disarm", false);
    ap.cron_job_id = Some("ghost-job".to_string());
    super::disarm_autopilot_job(&ctx, &ap);

    // 毒化：子线程持锁 panic → Mutex 中毒；disarm 走 Err WARN 臂。
    let poison = cron.clone();
    let _ = std::thread::spawn(move || {
        let _g = poison.lock().unwrap();
        panic!("poison cron lock");
    })
    .join();
    let mut ap2 = p3_ap(&store, "w5-disarm-2", false);
    ap2.cron_job_id = Some("ghost-job-2".to_string());
    super::disarm_autopilot_job(&ctx, &ap2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// issue.dispatch 正路径（316-322）：显式 target + peer 在线 + RPC
/// client → Ok(Some) 响应带 task_id、单转 in_progress、指派回填（762-771）；
/// RPC spawn 体对不可达对端诚实终结（⛔ 派发失败评论 + FAILED）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_issue_dispatch_success_via_handler() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-dispatch-ok");
    let cluster = offline_cluster(&dir, "w5-d-c");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-py", &[], &[]);
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        Some(cluster),
        None,
        None,
    );
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "派发正路径".into(),
            ..Default::default()
        })
        .unwrap();
    let out = dispatch(
        &ctx,
        "issue.dispatch",
        serde_json::json!({ "id": issue.id, "target": "node-py" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["dispatched"], serde_json::json!(true), "{out}");
    assert!(!out["task_id"].as_str().unwrap().is_empty());
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
    // 派发即指派回填。
    assert_eq!(
        store.get_issue(issue.id).unwrap().assignee_id.as_deref(),
        Some("node-py")
    );

    // RPC spawn 体：对端不可达 → finish_dispatch FAILED + ⛔ 评论（有界
    // 轮询等它落定）。
    let mut saw_fail = false;
    for _ in 0..60 {
        if store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("⛔ 派发失败"))
        {
            saw_fail = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_fail, "RPC spawn 体失败终结应落 ⛔ 评论");
    // F3 失败分类：不可达对端 → 评论带 [节点不可达] 标签。
    let fail_comment = store
        .list_comments(issue.id)
        .unwrap()
        .into_iter()
        .find(|c| c.content.contains("⛔ 派发失败"))
        .map(|c| c.content)
        .unwrap_or_default();
    assert!(
        fail_comment.contains("[节点不可达]"),
        "失败评论必须带分类标签：{fail_comment}"
    );
    // F2（2026-09-30）：送达失败 = 任务从未开始 → 状态回退派发前值 +
    // 指派恢复 + dispatch_rolled_back 审计（rollback 在 ⛔ 评论之前执行，
    // 评论已现即回退已落定，无需再轮询）。
    let rolled = store.get_issue(issue.id).unwrap();
    assert_eq!(
        rolled.status,
        IssueStatus::Backlog,
        "送达失败应回退到派发前状态"
    );
    assert_eq!(rolled.assignee_id, None, "指派应恢复派发前值（无指派）");
    assert!(
        store
            .list_activity(issue.id)
            .unwrap()
            .iter()
            .any(|a| a.action == "dispatch_rolled_back"),
        "回退必须落 dispatch_rolled_back 审计"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// D0b 重平衡（2153-2268）：空队列早退（2175-2176）→ 非超载源跳过
/// （2196-2198）→ 超载源（在途 2 > cap 1）队首搬给空闲目标：旧排队单
/// cancel + 重派成功 + ↳ 评论（2191-2266）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_rebalance_moves_queued_from_overloaded_worker() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-rebalance");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let actor = Actor::admin("w5");
    let cluster = offline_cluster(&dir, "w5-rb-c");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-tgt", &[], &[]);
    let cfg = nemesis_config::BoardFlagConfig {
        worker_max_inflight: 1,
        ..Default::default()
    };

    // 空队列：无处可挪（2175-2176）。
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-tgt", &actor).await,
        0
    );

    let mk = |t: &str| {
        store
            .create_issue(nemesis_board::NewIssue {
                title: t.into(),
                ..Default::default()
            })
            .unwrap()
    };
    let x = mk("重平衡甲");
    let y = mk("重平衡乙");
    store
        .insert_dispatch("w5-rb-x", x.id, "node-src", &actor)
        .unwrap();

    // 源在途 1 = cap → 非超载，排队单不动（2196-2198）。
    assert_eq!(
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-tgt", &actor).await,
        0
    );

    // 源在途 2 > cap → 队首搬给 node-tgt（free=1 只搬一单）。
    store
        .insert_dispatch("w5-rb-y", y.id, "node-src", &actor)
        .unwrap();
    let moved =
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-tgt", &actor).await;
    assert_eq!(moved, 1, "free=1 只搬一单");
    let comments = store.list_comments(x.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("重平衡")),
        "{comments:?}"
    );
    // 甲已重派在途（新 task 挂 node-tgt），乙排队单保留原状。
    assert!(store.has_active_dispatch(x.id).unwrap());
    assert!(store.has_active_dispatch(y.id).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

/// sweep 失败计数（2134-2140）：匹配 peer 上线但集群缺 RPC client →
/// 候选派发核心失败 → failed=1（诚实计数，不静默吞）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_sweep_counts_failed_candidate_dispatch() {
    let dir = unique_dir("w5-sweep-fail");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let cluster = offline_cluster(&dir, "w5-sf-c");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "sweep 失败父单".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "子A", vec!["python".to_string()]);

    // 先停车（无节点）→ 候选集有它。
    super::dispatch_subissue_auto(&store, Some(&cluster), a.id, &actor, true).unwrap();
    // 匹配节点上线但**不注入** RPC client → sweep 重试走派发核心 → 失败。
    w5_online_peer(&cluster, "node-py", &["python"], &[]);
    let (cands, dispatched, failed) = super::sweep_parked_dispatches(&store, &cluster, &actor);
    assert_eq!((cands, dispatched), (1, 0));
    assert_eq!(failed, 1, "缺 RPC client 的派发失败必须入 failed 计数");
    let _ = std::fs::remove_dir_all(&dir);
}

/// issue.cancel 带在途派发 + RPC client → spawn_task_cancel 走 spawn 下行
/// 路径（2677-2691）；channel.post 带 reply_to/kind_tag（4185-4196）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_cancel_active_dispatch_spawns_rpc_and_channel_post_reply_to() {
    // --- cancel：在途单 + RPC client → task_cancel spawn ---
    let dir = unique_dir("w5-cancel-rpc");
    let cluster = offline_cluster(&dir, "w5-cc-c");
    w5_inject_rpc(&cluster);
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        Some(cluster),
        None,
        None,
    );
    let actor = Actor::admin("w5");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "在途取消RPC".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("w5-task-cc", issue.id, "node-b", &actor)
        .unwrap();
    let out = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": issue.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], serde_json::json!(true), "{out}");
    assert_eq!(out["task_id"], serde_json::json!("w5-task-cc"));
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::Cancelled
    );
    // 放出调度权让 cancel spawn 体出闸（终结结果不 assert——对端不可达）。
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let _ = std::fs::remove_dir_all(&dir);

    // --- channel.post：reply_to + kind_tag 经桥转发 ---
    let dir2 = unique_dir("w5-channel-reply");
    let store2 = Arc::new(BoardStore::open(&dir2.join("board.db"), "NB").unwrap());
    store2.ensure_default_channels().unwrap();
    let fake = Arc::new(FakeIngress(std::sync::Mutex::new(Vec::new())));
    let ctx2 = make_ctx_with_service(
        &dir2,
        nemesis_board::BoardService::new(store2.clone(), NodeRole::Coordinator)
            .with_discussion(fake.clone()),
    );
    let cid = ctx2
        .state
        .board
        .as_ref()
        .unwrap()
        .store()
        .list_channels()
        .unwrap()[0]
        .id;
    let out = dispatch(
        &ctx2,
        "channel.post",
        serde_json::json!({
            "channel_id": cid,
            "content": "带引用的发言",
            "reply_to": 7,
            "kind_tag": "question",
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["posted"]["message_id"], serde_json::json!(42));
    {
        let calls = fake.0.lock().unwrap();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].2, cid);
        assert_eq!(calls[0].3, "带引用的发言");
    }
    let _ = std::fs::remove_dir_all(&dir2);
}

/// handler 杂项臂合集：bulk_archive 非数组 ids / autopilot.create+update
/// 全字段 / comment.add parent_id / project.progress 单项目 / project.resume
/// 缺参+dry_run / estop 拒 resume / project.open_dir 缺参 / attachment 读
/// 失败 / config.get 坏 config.json。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_handler_misc_arms() {
    let dir = unique_dir("w5-misc");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);

    // bulk_archive：ids 非数组 → 解析拒绝（3613-3615）。
    let err = dispatch(
        &ctx,
        "issue.bulk_archive",
        serde_json::json!({ "ids": "nope" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("ids 应为数字数组"), "{err}");

    // autopilot.create 全字段（3639-3658）。
    let proj = store
        .create_project(
            "w5 项目",
            "desc",
            None,
            "",
            "",
            Some(&dir.join("bp-w5").to_string_lossy()),
        )
        .unwrap();
    let created = dispatch(
        &ctx,
        "autopilot.create",
        serde_json::json!({
            "name": "全字段", "cron": "0 9 * * *", "title": "标题 {date}",
            "description": "描述", "priority": 1, "project_id": proj.id,
            "target": "", "enabled": true, "auto_plan": true,
            "acceptance_criteria": "报告含验收项",
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(created["autopilot"]["priority"], serde_json::json!(1));
    assert_eq!(
        created["autopilot"]["project_id"],
        serde_json::json!(proj.id)
    );
    assert_eq!(created["autopilot"]["auto_plan"], serde_json::json!(true));
    assert_eq!(
        created["autopilot"]["acceptance_criteria"],
        serde_json::json!("报告含验收项")
    );
    let ap_id = created["autopilot"]["id"].as_i64().unwrap();

    // autopilot.update 全字段（3684-3691）。
    let updated = dispatch(
        &ctx,
        "autopilot.update",
        serde_json::json!({
            "id": ap_id, "priority": 2, "target": "node-ghost",
            "auto_plan": false, "acceptance_criteria": "",
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated["autopilot"]["priority"], serde_json::json!(2));
    assert_eq!(updated["autopilot"]["auto_plan"], serde_json::json!(false));
    assert_eq!(
        updated["autopilot"]["target"],
        serde_json::json!("node-ghost")
    );

    // comment.add 带 parent_id（3787）。
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "评论父单".into(),
            ..Default::default()
        })
        .unwrap();
    let root = dispatch(
        &ctx,
        "comment.add",
        serde_json::json!({ "issue_id": issue.id, "content": "根评论" }),
    )
    .await
    .unwrap()
    .unwrap();
    let root_id = root["comment"]["id"].as_i64().unwrap();
    let reply = dispatch(
        &ctx,
        "comment.add",
        serde_json::json!({ "issue_id": issue.id, "content": "子评论", "parent_id": root_id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reply["comment"]["parent_id"], serde_json::json!(root_id));

    // project.progress 带 project_id → 单项目聚合（3852-3857）。
    let prog = dispatch(
        &ctx,
        "project.progress",
        serde_json::json!({ "project_id": proj.id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(prog.is_object(), "{prog}");

    // project.resume：缺 project_id（3864-3868）；dry_run 解析臂（3869-3872）
    // → 无集群拒绝。
    let err = dispatch(&ctx, "project.resume", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("missing field: project_id"), "{err}");
    let err = dispatch(
        &ctx,
        "project.resume",
        serde_json::json!({ "project_id": proj.id, "dry_run": true }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("集群未运行"), "{err}");

    // estop 生效 → resume 护栏拒绝（3874-3881）。
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let mut estopped = (*ctx.state).clone();
    estopped.estop = Some(estop);
    let ctx_e = RequestContext {
        session_id: ctx.session_id.clone(),
        chat_id: ctx.chat_id.clone(),
        workspace: ctx.workspace.clone(),
        home: ctx.home.clone(),
        state: Arc::new(estopped),
        auth_method: ctx.auth_method.clone(),
    };
    let err = dispatch(
        &ctx_e,
        "project.resume",
        serde_json::json!({ "project_id": proj.id }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("急停（E-STOP）"), "{err}");

    // project.open_dir：缺 project_id（4026-4029）。
    let err = dispatch(&ctx, "project.open_dir", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("missing field: project_id"), "{err}");

    // attachment：add 后删文件 → get 读文件失败（4090-4093）。
    let added = dispatch(
        &ctx,
        "attachment.add",
        serde_json::json!({ "issue_id": issue.id, "filename": "note.txt", "content": "aGVsbG8=" }),
    )
    .await
    .unwrap()
    .unwrap();
    let att_id = added["attachment"]["id"].as_i64().unwrap();
    std::fs::remove_dir_all(dir.join("board").join("files")).unwrap();
    let err = dispatch(&ctx, "attachment.get", serde_json::json!({ "id": att_id }))
        .await
        .unwrap_err();
    assert!(err.contains("读取附件文件失败"), "{err}");

    // config.get：home 下坏 config.json → 读取失败透传（4260-4263）。
    std::fs::write(dir.join("config.json"), "{invalid").unwrap();
    let err = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("config.json 读取失败"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// execute_plan_chain 集群画像注入（1414-1434）：有在线节点 → planner
/// prompt 带节点画像（tags「—」/空 caps 与非空两态都渲染）→ planned +
/// planner done 收尾（1461-1465）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_plan_chain_renders_cluster_profile() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-chain-profile");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let hub = EventHub::new();
    let cluster = offline_cluster(&dir, "w5-cp-c");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-py", &["python"], &["cpu"]);
    w5_online_peer(&cluster, "node-bare", &[], &[]);
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "画像链父单".into(),
            ..Default::default()
        })
        .unwrap();
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtGoodPlannerProvider),
        AgentConfig::default(),
    ));
    let out = super::execute_plan_chain(
        &store,
        Some(cluster),
        al,
        parent.clone(),
        actor,
        "w5-chain-cp",
        None,
        Some(&hub),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out["status"], serde_json::json!("planned"), "{out}");
    assert_eq!(out["subs"], serde_json::json!(2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 一段拆解 spawn 链失败（1622-1637）：BadProvider → 后台链三连败 →
/// board.plan_failed 事件 + 链内收尾 warn（1636）。事件订阅做确定性观察。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_spawned_plan_chain_failure_publishes_event() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-plan1-fail");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtBadPlannerProvider),
        AgentConfig::default(),
    ));
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        None,
        Some(al),
        None,
    );
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "失败链单".into(),
            ..Default::default()
        })
        .unwrap();
    let mut rx = ctx.state.event_hub.subscribe();
    let out = dispatch(&ctx, "issue.plan", serde_json::json!({ "id": issue.id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], serde_json::json!("planning"));
    let mut got = false;
    for _ in 0..100 {
        match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(ev)) if ev.event_type == "board.plan_failed" => {
                got = true;
                break;
            }
            Ok(Ok(_)) => continue,
            _ => continue,
        }
    }
    assert!(got, "后台链失败必须发布 board.plan_failed");
    // 收尾 warn 在事件发布之后落——再放几拍调度权（无断言）。
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 项目自动开工（3084-3148）：agent 在 → 建父单 + spawn 链跑完（预览按
/// issue 入缓存，3123-3147）；空项目名 → create_issue `?` 诚实失败
///（3093-3104）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_project_auto_start_with_agent_and_empty_name() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-autostart");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let al = Arc::new(AgentLoop::new(
        Box::new(AgtGoodPlannerProvider),
        AgentConfig::default(),
    ));
    let ctx = agt_make_ctx_ex(
        &dir,
        nemesis_board::BoardService::new(store.clone(), NodeRole::Coordinator),
        None,
        Some(al),
        None,
    );
    let actor = Actor::admin("w5");
    let proj = store
        .create_project(
            "w5 开工项目",
            "desc",
            None,
            "",
            "",
            Some(&dir.join("bp-w5-start").to_string_lossy()),
        )
        .unwrap();

    // 空名 → create_issue 失败透传。
    let err =
        super::spawn_project_auto_start(&store, &ctx, &actor, "", "d", None, proj.id).unwrap_err();
    assert!(err.contains("title must not be empty"), "{err}");

    // 正常：父单建出 + 后台链写预览（按 issue_id 定位，不受并行测试的
    // 全局 PLAN_CACHE 污染）。
    let issue = super::spawn_project_auto_start(
        &store,
        &ctx,
        &actor,
        "w5 开工项目",
        "desc",
        Some("有回归".to_string()),
        proj.id,
    )
    .unwrap();
    assert_eq!(issue.project_id, Some(proj.id));
    let mut chained = false;
    for _ in 0..100 {
        if super::PLAN_CACHE
            .lock()
            .values()
            .any(|p| p.issue_id == issue.id)
        {
            chained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(chained, "后台链应把预览写进缓存 issue={}", issue.id);
    let _ = std::fs::remove_dir_all(&dir);
}

/// D2 auto_plan 有 moderator 槽（3184-3214）：同步段装配 spawn 返回
/// planning + plan_id；后台链 GoodPlanner → 预览按 plan_id 入缓存。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_fire_autopilot_plan_chain_filled_slot() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-ap-slot");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("w5");
    let ap = p3_ap(&store, "w5-slot", true);
    let slot = Arc::new(std::sync::OnceLock::new());
    assert!(
        slot.set(Arc::new(AgentLoop::new(
            Box::new(AgtGoodPlannerProvider),
            AgentConfig::default(),
        )))
        .is_ok(),
        "槽应空，装配一次即成功"
    );
    let ap_ctx = super::AutoPlanContext {
        moderator_slot: slot,
        home: dir.clone(),
        hub: Some(Arc::new(EventHub::new())),
        cluster: None,
    };
    let out = super::fire_autopilot(&store, None, &ap, &actor, Some(&ap_ctx)).unwrap();
    assert_eq!(out["ran"], serde_json::json!(true), "{out}");
    assert_eq!(
        out["auto_plan"]["status"],
        serde_json::json!("planning"),
        "{out}"
    );
    let plan_id = out["auto_plan"]["plan_id"].as_str().unwrap().to_string();
    let mut chained = false;
    for _ in 0..100 {
        if super::PLAN_CACHE.lock().contains_key(&plan_id) {
            chained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(chained, "后台链应把预览写进缓存 plan_id={plan_id}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 任务资产段（328-341）：签发上下文未注入 → None 早退；注入后（无资产）
/// 渲染入口执行并诚实降级 Ok(None)——335-340 全臂执行。
#[cfg(feature = "cluster")]
#[test]
fn w5_render_assets_section_with_signing_context() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律；
    // 同步测试走 blocking_lock，无 runtime 不炸）。
    let _serial = PLAN_TEST_LOCK.blocking_lock();
    clear_plan_globals();
    let dir = unique_dir("w5-assets");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "资产单".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        super::render_dispatch_assets_section(&store, issue.id).unwrap(),
        None,
        "未注入 → None 早退"
    );
    store.set_asset_signing(nemesis_board::asset_token::AssetSignContext {
        secret: vec![1, 2, 3],
        node_url: Default::default(),
        node_id: "w5-node".to_string(),
    });
    assert_eq!(
        super::render_dispatch_assets_section(&store, issue.id).unwrap(),
        None,
        "注入后无资产 → Ok(None)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 集群专业职能框架 M2：confirm_plan 降级评论 + required_profession 落库
// ---------------------------------------------------------------------------

/// D9 末轮宽和降级：downgrades 明细逐条落被降级子单的系统评论；合法职能
/// 原样落库。
#[cfg(feature = "cluster")]
#[test]
fn confirm_plan_writes_downgrade_comments_and_keeps_valid_professions() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律；
    // 同步测试走 blocking_lock，无 runtime 不炸）。
    let _serial = PLAN_TEST_LOCK.blocking_lock();
    clear_plan_globals();
    let dir = unique_dir("confirm-downgrade-comments");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "降级父单".to_string(),
            creator: nemesis_board::Actor::system("board"),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();
    let subs = vec![
        nemesis_board::PlannedSubIssue {
            title: "被降级子单".to_string(),
            required_profession: "BAD SLUG".to_string(),
            required_capabilities: vec![],
            ..nemesis_board::PlannedSubIssue::default()
        },
        nemesis_board::PlannedSubIssue {
            title: "合法职能子单".to_string(),
            required_profession: "dev:cpp".to_string(),
            required_capabilities: vec![],
            ..nemesis_board::PlannedSubIssue::default()
        },
    ];
    let downgrades = vec![(0usize, "BAD SLUG".to_string())];
    let out = super::confirm_plan(
        &store,
        None, // 集群未装配 → 派发诚实降级（不撞本断言关注点）
        &parent,
        subs,
        &nemesis_board::Actor::system("board"),
        &downgrades,
    )
    .unwrap();
    let created: Vec<i64> = out["created"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();

    // 子 0：职能置空落库 + 系统评论留痕。
    let sub0 = store.get_issue(created[0]).unwrap();
    assert_eq!(sub0.required_profession, None, "降级子单职能 = 不限");
    let comments0 = store.list_comments(created[0]).unwrap();
    assert!(
        comments0
            .iter()
            .any(|c| c.content.contains("required_profession=`BAD SLUG`")
                && c.content.contains("已降级无职能")),
        "降级评论必须落在被降级子单上: {comments0:?}"
    );

    // 子 1：合法职能原样落库、无降级评论。
    let sub1 = store.get_issue(created[1]).unwrap();
    assert_eq!(sub1.required_profession.as_deref(), Some("dev:cpp"));
    let comments1 = store.list_comments(created[1]).unwrap();
    assert!(
        !comments1.iter().any(|c| c.content.contains("已降级无职能")),
        "合法子单不应有降级评论: {comments1:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// file: 锚点拓扑约束回归锁（2026-10-08 NB-4/NB-5/NB-6 根修）。谓词矩阵在
// nemesis-board anchor/tests.rs；此处钉三个决策面的行为：重平衡不偷拓扑受
// 限单 / 重派失败补偿评论 / 匹配器与兜底对 file: 锚点单返回空候选。
// ===========================================================================

/// 重平衡偷单过滤：超载源两条排队单，file: 锚点的必须原地保留（转派远端
/// 必被拓扑闸拒绝且 cancel 先行不可逆），无锚点的照常搬走——过滤必须是
/// 选择性的，不是一刀切。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_rebalance_skips_file_anchor_queued_but_moves_plain() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-rb-file");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let actor = Actor::admin("w5");
    let cluster = offline_cluster(&dir, "w5-rbf-c");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-tgt", &[], &[]);
    let cfg = nemesis_config::BoardFlagConfig {
        worker_max_inflight: 1,
        ..Default::default()
    };

    let mk = |t: &str, ac: Option<&str>| {
        store
            .create_issue(nemesis_board::NewIssue {
                title: t.into(),
                acceptance_criteria: ac.map(str::to_string),
                ..Default::default()
            })
            .unwrap()
    };
    // 甲：file: 锚点（拓扑受限）；乙：无锚点（可转派）。同挂超载源。
    let file_anchored = mk(
        "拓扑受限甲",
        Some("[CHECK] file:deliverables/plan.md exists"),
    );
    let plain = mk("普通乙", None);
    store
        .insert_dispatch("w5-rbf-x", file_anchored.id, "node-src", &actor)
        .unwrap();
    store
        .insert_dispatch("w5-rbf-y", plain.id, "node-src", &actor)
        .unwrap();

    let moved =
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-tgt", &actor).await;
    assert_eq!(
        moved, 1,
        "file: 锚点单跳过、无锚点单搬走（free=1 只搬一单）"
    );

    // 甲：排队派发原地保留（仍活跃、未被 cancel、无补偿评论——它根本没
    // 进偷单流程）。
    assert!(
        store.has_active_dispatch(file_anchored.id).unwrap(),
        "file: 锚点排队单必须原地保留"
    );
    let kept = store.list_dispatches(file_anchored.id).unwrap();
    assert!(
        kept.iter().any(|d| d.task_id == "w5-rbf-x"),
        "原派发不得被取消: {kept:?}"
    );
    assert!(
        !store
            .list_comments(file_anchored.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("重平衡")),
        "拓扑跳过不是异常，不得落补偿评论"
    );

    // 乙：正常重平衡（新派发挂 node-tgt + ↳ 评论）。
    assert!(store.has_active_dispatch(plain.id).unwrap());
    assert!(
        store
            .list_comments(plain.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("转派至 node-tgt")),
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 重派失败补偿（NB-5/NB-6 事故面的最小重演）：排队单被 cancel 后重派因
/// 状态闸失败（issue 已 Blocked）→ moved=0 + ⛔ 系统评论诚实留痕 + issue
/// 无活跃派发——「无声死透」必须变成「可见停车」。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn w5_rebalance_failure_leaves_compensation_comment() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("w5-rb-comp");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let actor = Actor::admin("w5");
    let cluster = offline_cluster(&dir, "w5-rbc-c");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-tgt", &[], &[]);
    let cfg = nemesis_config::BoardFlagConfig {
        worker_max_inflight: 1,
        ..Default::default()
    };

    // 垫行：file: 锚点排队单把 node-src 推成超载（load 2 > cap 1）且自身
    // 被拓扑过滤跳过（不消耗 free）——补偿测试只需关注 Blocked 单。
    let shim = store
        .create_issue(nemesis_board::NewIssue {
            title: "垫行（拓扑受限）".into(),
            acceptance_criteria: Some("[CHECK] file:deliverables/plan.md exists".into()),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("w5-rbc-shim", shim.id, "node-src", &actor)
        .unwrap();

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "补偿面".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("w5-rbc-x", issue.id, "node-src", &actor)
        .unwrap();
    // Backlog → Blocked 合法转移：制造「排队单在、issue 不可派」的组合
    // （真机形态：cancel 落库后重派被状态闸拒绝）。
    store
        .transition_issue(issue.id, IssueStatus::Blocked, &actor)
        .unwrap();

    let moved =
        super::rebalance_queued_to_worker(&store, &cluster, Some(&cfg), "node-tgt", &actor).await;
    assert_eq!(moved, 0, "重派失败不得计入 moved");

    // 原排队派发已被 cancel（不可逆），issue 无活跃派发。
    let dispatches = store.list_dispatches(issue.id).unwrap();
    assert!(
        dispatches.iter().all(|d| d.task_id != "w5-rbc-x"
            || d.state == nemesis_board::models::dispatch_state::CANCELLED),
        "原排队派发必须已终态: {dispatches:?}"
    );
    assert!(!store.has_active_dispatch(issue.id).unwrap());

    // 补偿评论：失败原因 + 现状 + 可重派指引。
    let comments = store.list_comments(issue.id).unwrap();
    let comp = comments
        .iter()
        .find(|c| c.content.contains("D0b 重平衡转派失败"))
        .expect("必须有 ⛔ 补偿评论");
    assert!(
        comp.content.contains("node-src"),
        "评论须带原 worker: {comp:?}"
    );
    assert!(
        comp.content.contains("已被取消且无法自动恢复"),
        "评论须诚实说明不可逆: {comp:?}"
    );
    // 垫行：拓扑跳过、原地保留、无补偿评论。
    assert!(store.has_active_dispatch(shim.id).unwrap());
    assert!(
        !store
            .list_comments(shim.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("重平衡"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 匹配器与兜底两个决策面对 file: 锚点单必须返回空（远端候选注定被拓扑
/// 闸拒绝，不得产出非法目标）；无锚点对照臂照常产出候选。
#[cfg(feature = "cluster")]
#[test]
fn w5_rank_and_fallback_return_empty_for_file_anchor_issue() {
    let dir = unique_dir("w5-rank-file");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let cluster = offline_cluster(&dir, "w5-rf-c");
    w5_inject_rpc(&cluster);
    w5_online_peer(&cluster, "node-qa", &["qa"], &[]);

    let file_anchored = bare_issue(Some("qa"), &[]);
    let mut anchored = file_anchored.clone();
    anchored.acceptance_criteria = Some("[CHECK] file:deliverables/spec.md exists".to_string());

    // rank_dispatch_candidates：file: 锚点 → 空候选（即使匹配节点在线）。
    assert!(
        super::rank_dispatch_candidates(&store, &cluster, &anchored).is_empty(),
        "file: 锚点单必须返回空候选"
    );
    // 对照：同单去掉锚点 → 候选恢复（过滤是锚点驱动的，不是别的什么）。
    assert_eq!(
        super::rank_dispatch_candidates(&store, &cluster, &file_anchored),
        vec!["node-qa".to_string()]
    );

    // pick_fallback_target：file: 锚点 → None（兜底产物全是远端节点）。
    let cfg = nemesis_config::BoardFlagConfig::default();
    assert!(super::pick_fallback_target(Some(&cfg), &store, &cluster, &anchored).is_none());
    // 对照：无锚点单兜底照常（在线节点存在 → Some）。
    assert!(super::pick_fallback_target(Some(&cfg), &store, &cluster, &file_anchored).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

// M6-2（2026-10-10 机制加固，P5 clobber 根修）：rebase_board_onto_disk——
// 磁盘为底只嫁接 board 段；窗口期外部写者的非 board 改动不得被回滚。
#[test]
fn test_rebase_board_onto_disk_preserves_external_writes() {
    let dir = unique_dir("config-rebase");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
    // 磁盘 v1：外部写者写入的 model_list（m1）。
    std::fs::write(
        &path,
        r#"{"model_list":[{"model_name":"m1","model":"prov/m1","api_key":"sk-1"}]}"#,
    )
    .unwrap();
    // 陈旧快照（live cache 形态）：快照里还是 m1。
    let stale: nemesis_config::Config =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    // 窗口期：外部写者把磁盘改到 v2（m1 → m2，模拟 CLI model add 换键）。
    std::fs::write(
        &path,
        r#"{"model_list":[{"model_name":"m2","model":"prov/m2","api_key":"sk-2"}]}"#,
    )
    .unwrap();
    // 本次 board.config.set 的产物 = 陈旧快照 + 新 board 值。
    let mut mutated = stale.clone();
    let board = mutated.board.get_or_insert_with(Default::default);
    board.unlimited_mode = true;

    let merged = super::rebase_board_onto_disk(mutated, &path).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&merged).unwrap()).unwrap();
    assert_eq!(
        raw["model_list"][0]["model_name"], "m2",
        "窗口期外部写者的非 board 改动不得被整份回写回滚（P5 clobber 根修）"
    );
    assert_eq!(
        raw["board"]["unlimited_mode"], true,
        "本次 board 段新值必须保留"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// M7（2026-10-10）：exec 锚点开关/超时进 config.set 白名单；get 序列化
// typed board 段自动透出。
#[tokio::test]
async fn test_board_config_set_exec_anchor_keys() {
    let dir = unique_dir("config-exec-anchor");
    let ctx = make_ctx_with_board(&dir);

    let out = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.exec_anchors_enabled", "value": true }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    let out = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.exec_anchor_timeout_secs", "value": 60 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);

    let out = dispatch(&ctx, "config.get", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["review"]["exec_anchors_enabled"], true);
    assert_eq!(out["review"]["exec_anchor_timeout_secs"], 60);

    // 盘上真实落盘（rebase 后 board 段照常落）。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["board"]["review"]["exec_anchors_enabled"], true);
    assert_eq!(raw["board"]["review"]["exec_anchor_timeout_secs"], 60);

    // 超时键非负整数闸。
    let err = dispatch(
        &ctx,
        "config.set",
        serde_json::json!({ "key": "review.exec_anchor_timeout_secs", "value": "soon" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("非负整数"), "实际报错：{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P11 观测面（2026-10-10）：plan 预览查询（plan.list/plan.get）+ planner
// 结果评论留痕（execute_plan_chain 三发布点）
// ---------------------------------------------------------------------------

/// planner 链测试用迷你 provider（同 nemesis-agent spawn_detached 形状）：
/// `content=Some` 输出固定文本（放合法 plan JSON 走成功链），`None` 恒 Err
///（走 LLM 调用失败臂）。
#[cfg(feature = "cluster")]
struct PlanChainMockProvider {
    content: Option<String>,
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for PlanChainMockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        match &self.content {
            Some(c) => Ok(nemesis_agent::r#loop::LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: c.clone(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            }),
            None => Err("provider exploded".to_string()),
        }
    }
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_query_list_and_get_full_fields() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-query-full");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "P11 观测父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let mut subs = vec![sub("子单甲", vec![]), sub("子单乙", vec![0])];
    subs[0].required_role = "developer".to_string();
    subs[1].required_profession = "dev:cpp".to_string();
    super::PLAN_CACHE.lock().remove("pl-a3-full"); // 防御性清场（static 进程级共享）
    super::PLAN_CACHE.lock().insert(
        "pl-a3-full".to_string(),
        super::PlanPreview {
            issue_id: issue.id,
            subs: subs.clone(),
            downgrades: vec![(1, "dev:cpp:坏的".to_string())],
            created_at: Instant::now(),
        },
    );

    // list（按 issue 过滤）全字段投影。
    let out = dispatch(
        &ctx,
        "plan.list",
        serde_json::json!({ "issue_id": issue.id }),
    )
    .await
    .unwrap()
    .unwrap();
    let mine: Vec<_> = out["plans"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["plan_id"] == "pl-a3-full")
        .collect();
    assert_eq!(mine.len(), 1, "该 issue 恰一条: {out}");
    let p = mine[0];
    assert_eq!(p["issue_id"], issue.id);
    assert_eq!(p["issue_number"], issue.number, "issue_number 应附属投影");
    assert_eq!(p["sub_count"], 2);
    assert_eq!(p["subs"][0]["title"], "子单甲");
    assert_eq!(p["subs"][0]["required_role"], "developer");
    assert_eq!(p["subs"][1]["required_profession"], "dev:cpp");
    assert_eq!(p["subs"][1]["depends_on"], serde_json::json!([0]));
    assert_eq!(
        p["downgrades"],
        serde_json::json!([[1, "dev:cpp:坏的"]]),
        "降级明细应随预览带出"
    );
    let ttl = p["ttl_remaining_secs"].as_u64().unwrap();
    assert!(ttl > 590 && ttl <= 600, "ttl 应贴近 600 上限: {ttl}");
    assert!(
        p["created_ms"].as_u64().unwrap() < 5_000,
        "刚种入的条目 elapsed 应极小"
    );

    // get 单查同投影。
    let out = dispatch(
        &ctx,
        "plan.get",
        serde_json::json!({ "plan_id": "pl-a3-full" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["plan_id"], "pl-a3-full");
    assert_eq!(out["sub_count"], 2);
    assert_eq!(out["ttl_remaining_secs"], p["ttl_remaining_secs"]);

    // list 无过滤含本条；别的 issue 过滤不串台。
    let out = dispatch(&ctx, "plan.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert!(
        out["plans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["plan_id"] == "pl-a3-full")
    );
    let out = dispatch(
        &ctx,
        "plan.list",
        serde_json::json!({ "issue_id": issue.id + 999 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        !out["plans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["plan_id"] == "pl-a3-full")
    );

    super::PLAN_CACHE.lock().remove("pl-a3-full");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_query_missing_and_expired_states() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-query-states");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "P11 状态父单".to_string(),
            ..Default::default()
        })
        .unwrap();

    // ① 不存在 → 诚实报错（不静默空对象）。
    let err = dispatch(
        &ctx,
        "plan.get",
        serde_json::json!({ "plan_id": "pl-a3-nope" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("不存在、已过期或已消费"), "{err}");

    // ② 过期 → 报错 + 缓存条目顺带摘除；list 只留存活（TTL 惰性过滤）。
    // 回拨 created_at 需 Instant 纪元余量；刚启动系统回拨不了 → 环境边界
    // 诚实跳过（探针纪律：依赖外部环境的前提不满足不硬跑）。
    let Some(stale) = Instant::now().checked_sub(std::time::Duration::from_secs(700)) else {
        eprintln!("SKIP 过期态断言：系统刚启动，Instant 纪元余量不足 700s");
        return;
    };
    super::PLAN_CACHE.lock().insert(
        "pl-a3-stale".to_string(),
        super::PlanPreview {
            issue_id: issue.id,
            subs: vec![sub("过期子单", vec![])],
            downgrades: Vec::new(),
            created_at: stale,
        },
    );
    seed_plan("pl-a3-fresh", issue.id, vec![sub("新鲜子单", vec![])]);
    let err = dispatch(
        &ctx,
        "plan.get",
        serde_json::json!({ "plan_id": "pl-a3-stale" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("过期"), "应指明过期: {err}");
    assert!(
        super::PLAN_CACHE.lock().get("pl-a3-stale").is_none(),
        "过期条目应被 get 顺带摘除"
    );

    let out = dispatch(
        &ctx,
        "plan.list",
        serde_json::json!({ "issue_id": issue.id }),
    )
    .await
    .unwrap()
    .unwrap();
    let ids: Vec<&str> = out["plans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["plan_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"pl-a3-fresh"), "存活条目应返回: {ids:?}");
    assert!(!ids.contains(&"pl-a3-stale"), "过期条目应被过滤: {ids:?}");
    assert!(
        super::PLAN_CACHE.lock().get("pl-a3-fresh").is_some(),
        "list 不得误删存活条目"
    );

    super::PLAN_CACHE.lock().remove("pl-a3-fresh");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn planner_chain_success_comment_trace() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    // 一段拆解成功（mock provider 输出合法 plan JSON，home 无 config.json →
    // auto_confirm fail-closed 关）→ 预览在缓存 + 完成系统评论带 plan_id/
    // 子单数/TTL 提示。
    let dir = unique_dir("planner-success-comment");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "链路成功父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let plan_json =
        r#"[{"title":"子单甲","depends_on":[]},{"title":"子单乙","depends_on":[0]}]"#.to_string();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider {
            content: Some(plan_json),
        }),
        nemesis_agent::AgentConfig::default(),
    ));
    let hub = crate::events::EventHub::new();
    let mut rx = hub.subscribe();
    // 拆解运行期可观测（2026-10-11「停着」事故）回归：链开头应注册 running
    // 条目（in-flight 可查）——execute_plan_chain 同步 await 无法在运行中
    // 抽查，改为断言结束后条目已摘除（guard Drop 三退出路径中的正常路径）。
    let out = super::execute_plan_chain(
        &store,
        None, // 集群未装配 → 拆解照常，发车面不进本断言
        agent_loop,
        issue.clone(),
        nemesis_board::Actor::admin("tester"),
        "pl-a3-chain-ok",
        Some(&dir),
        Some(&hub),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out["status"], "planned", "{out}");
    assert_eq!(out["plan_id"], "pl-a3-chain-ok");
    assert!(
        super::PLAN_CACHE.lock().get("pl-a3-chain-ok").is_some(),
        "成功拆解应留预览待确认"
    );
    let comments = store.list_comments(issue.id).unwrap();
    let trace = comments
        .iter()
        .find(|c| c.content.contains("🧩 planner 拆解完成"))
        .expect("应有完成系统评论");
    assert!(
        trace.content.contains("pl-a3-chain-ok"),
        "{:?}",
        trace.content
    );
    assert!(trace.content.contains("2 个子单"), "{:?}", trace.content);
    assert!(
        trace.content.contains("600s 内确认派发"),
        "人工确认模式应带 TTL 提示: {:?}",
        trace.content
    );
    assert!(
        matches!(trace.ctype, nemesis_board::models::CommentType::System),
        "留痕应为系统评论"
    );
    // 开始评论（运行期持久通道）：与完成评论成对。
    let comments = store.list_comments(issue.id).unwrap();
    let started = comments
        .iter()
        .find(|c| c.content.contains("已开始自动拆解"))
        .expect("应有「已开始自动拆解」系统评论");
    assert!(
        started.content.contains("3~8 分钟"),
        "开始评论应带预期时长: {:?}",
        started.content
    );
    // board.plan_started 事件（运行期实时通道）+ issue_number 附属。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let got_started = loop {
        match rx.try_recv() {
            Ok(evt) => {
                assert_eq!(evt.event_type, "board.plan_started", "{:?}", evt.event_type);
                assert_eq!(evt.data["issue_id"], issue.id);
                assert_eq!(evt.data["plan_id"], "pl-a3-chain-ok");
                assert_eq!(evt.data["issue_number"], issue.number);
                break true;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "board.plan_started 事件未在 5s 内到达"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => panic!("事件通道异常: {e}"),
        }
    };
    let _ = got_started;
    // 正常结束后 running 条目必须摘除（guard Drop）。
    assert!(
        super::RUNNING_PLANS.lock().get("pl-a3-chain-ok").is_none(),
        "链结束后 running 条目应被摘除"
    );

    super::PLAN_CACHE.lock().remove("pl-a3-chain-ok");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn planner_chain_failure_comment_trace() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    // LLM 调用失败（mock 恒 Err）→ 失败系统评论带原因，缓存不残留。
    let dir = unique_dir("planner-fail-comment");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "链路失败父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider { content: None }),
        nemesis_agent::AgentConfig::default(),
    ));
    let err = super::execute_plan_chain(
        &store,
        None,
        agent_loop,
        issue.clone(),
        nemesis_board::Actor::admin("tester"),
        "pl-a3-chain-bad",
        Some(&dir),
        None, // hub=None：评论/落库链路照常（cron 路径同形态）
        1,
    )
    .await
    .unwrap_err();
    assert!(err.contains("planner LLM 调用失败"), "{err}");
    let comments = store.list_comments(issue.id).unwrap();
    let trace = comments
        .iter()
        .find(|c| c.content.contains("❌ planner 拆解失败"))
        .expect("应有失败系统评论");
    assert!(
        trace.content.contains("provider exploded"),
        "失败原因应留痕: {:?}",
        trace.content
    );
    assert!(
        super::PLAN_CACHE.lock().get("pl-a3-chain-bad").is_none(),
        "失败不得留缓存条目"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// panic 留痕闭环回归锁（2026-10-11 planner 截断 panic 事故）：后台 plan
/// 链任务 panic 时，监视壳必须往 issue 落系统评论 + 发 `board.plan_failed`
/// 事件——无声死亡（用户体感「建了不动」）不得再发生。
#[cfg(feature = "cluster")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_chain_panic_leaves_issue_comment_and_event() {
    // 写/读进程级 plan 全局——串行持锁 + 开场清残留（与派发闸测试同纪律）。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("plan-chain-panic-watch");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "panic 留痕回归锁".into(),
            ..Default::default()
        })
        .unwrap();
    let hub = ctx.state.event_hub.clone();
    let mut rx = hub.subscribe();

    super::spawn_plan_chain_watched(
        store.clone(),
        Some(hub.clone()),
        issue.id,
        "plan-watch-test".to_string(),
        async {
            panic!("截断中文 panic 靶点");
        },
    );

    // 轮询等监视任务落评论（任务 spawn 链两跳，不等固定时长）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let comments = store.list_comments(issue.id).unwrap();
        if comments
            .iter()
            .any(|c| c.content.contains("异常终止") && c.content.contains("panic 靶点"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "panic 评论未在 5s 内落盘"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // board.plan_failed 事件同发（监视壳在评论之后 publish，同样轮询防竞速）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let got = loop {
        match rx.try_recv() {
            Ok(evt) => {
                assert_eq!(evt.event_type, "board.plan_failed");
                assert_eq!(evt.data["issue_id"], issue.id);
                assert!(
                    evt.data["error"].as_str().unwrap().contains("panic"),
                    "error 应带 panic 摘要: {:?}",
                    evt.data["error"]
                );
                break true;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "board.plan_failed 事件未在 5s 内到达"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => panic!("事件通道异常: {e}"),
        }
    };
    let _ = got;
    let _ = std::fs::remove_dir_all(&dir);
}

/// plan.list running 态回归（2026-10-11「停着」事故）：in-flight 拆解必须
/// 可查（status=running + elapsed_secs），与 ready 条目并列——此前 PLAN_CACHE
/// 完成才写入，查询「正在跑」返回空，编排者无法区分拆解中与没跑。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_list_includes_running_entries() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-list-running");
    let ctx = make_ctx_with_board(&dir);
    // 65s 回拨需 Instant 纪元余量（elapsed_secs>=64 断言依赖真实历史时刻）；
    // 刚启动系统回拨不了 → 环境边界诚实跳过（同下方过期态断言纪律）。
    let Some(started) = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(65))
    else {
        eprintln!("SKIP running 投影断言：系统刚启动，Instant 纪元余量不足 65s");
        return;
    };
    super::RUNNING_PLANS.lock().insert(
        "pl-running-1".to_string(),
        super::RunningPlan {
            issue_id: 42,
            started_at: started,
            attempt: 1,
            phase: "planning",
        },
    );
    let out = dispatch(&ctx, "plan.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let plans = out["plans"].as_array().unwrap();
    // 按 plan_id 收敛断言（并行测试的在途链条目也在全局注册表里——跨 store
    // issue_id 撞号会混进列表，只断言自家条目在场且形态正确）。
    let mine: Vec<_> = plans
        .iter()
        .filter(|p| p["plan_id"] == "pl-running-1")
        .collect();
    assert_eq!(mine.len(), 1, "running 条目应出现在列表: {plans:?}");
    assert_eq!(mine[0]["status"], "running");
    assert_eq!(mine[0]["issue_id"], 42);
    assert!(
        mine[0]["elapsed_secs"].as_u64().unwrap() >= 64,
        "应带已等待秒数: {:?}",
        mine[0]
    );
    // issue_id 过滤同样命中。
    let out = dispatch(&ctx, "plan.list", serde_json::json!({ "issue_id": 42 }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["plans"].as_array().unwrap().len(), 1);
    let out = dispatch(&ctx, "plan.list", serde_json::json!({ "issue_id": 43 }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["plans"].as_array().unwrap().len(), 0);
    // plan.get 对 running 条目返回 running 投影（不再是「不存在」歧义）。
    let out = dispatch(
        &ctx,
        "plan.get",
        serde_json::json!({ "plan_id": "pl-running-1" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["status"], "running");
    assert_eq!(out["issue_id"], 42);
    super::RUNNING_PLANS.lock().remove("pl-running-1");
    let _ = std::fs::remove_dir_all(&dir);
}

/// project.create 同名友好错误回归（2026-10-11 用户实测）：项目名 UNIQUE
/// 约束此前透传裸 `UNIQUE constraint failed: project.name`——用户看不懂。
/// 预检后应为中文提示（含项目名 + 处理指引）。
#[tokio::test]
async fn project_create_duplicate_name_friendly_error() {
    let dir = unique_dir("project-dup-name");
    let ctx = make_ctx_with_board(&dir);
    let first = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "重名探针项目" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first["created"], true, "{first}");
    let err = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "重名探针项目" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("已存在同名项目"), "{err}");
    assert!(err.contains("重名探针项目"), "错误应带项目名: {err}");
    assert!(
        err.contains("唯一"),
        "错误应说明原因（项目名需唯一）: {err}"
    );
    // 归档后的同名项目同样占名（UNIQUE 不看 status），提示保持一致。
    let pid = first["project"]["id"].as_i64().unwrap();
    dispatch(
        &ctx,
        "project.update",
        serde_json::json!({ "id": pid, "status": "archived" }),
    )
    .await
    .unwrap()
    .unwrap();
    let err = dispatch(
        &ctx,
        "project.create",
        serde_json::json!({ "name": "重名探针项目" }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("已存在同名项目"),
        "归档同名也应被预检拦截: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// §1.1 拆解超时重拆链 / superseded 双闸 / N1 重启恢复（2026-10-11 计划 Part 1）
// ---------------------------------------------------------------------------

/// 本批 Part 1 测试串行闸：RUNNING_PLANS 是进程级全局，而各测试的独立
/// BoardStore 里 issue id 都从 1 起——并行跑会跨 store 误匹配 issue_id
///（让恢复扫描误判「在途链」/ issue.list 投影串台）。background_registry
/// TEST_LOCK 同款纪律。
///
/// BUG-② 派发闸（2026-10-15）后**读侧也入册**：`dispatch_issue_core` 会
/// 读 PLAN_CACHE/RUNNING_PLANS 拦「挂有未确认计划的父单」——所有走到
/// 派发路径的测试（含只读负例）必须持本锁，否则其他测试的残留条目按
/// issue id 撞库误拦（实录：9 个派发测试并行假红）。
static PLAN_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 派发闸读的两个进程级全局的开场清残留（cluster 门控；无 feature 编译为
/// 空调用）。持 PLAN_TEST_LOCK 期间调用——plan 链测试按设计会留条目
///（superseded mock 中途摘除后不回填等），不清则后续派发测试按 issue id
/// 撞库确定性误拦（实录：首批 9 个派发测试假红）。
#[cfg(feature = "cluster")]
fn clear_plan_globals() {
    super::PLAN_CACHE.lock().clear();
    super::RUNNING_PLANS.lock().clear();
}

#[cfg(not(feature = "cluster"))]
fn clear_plan_globals() {}

/// superseded 双闸专用 mock：LLM 返回前先摘 RUNNING_PLANS 条目——模拟超时
/// 监视已判定超时并摘除、旧链 LLM future 不可取消仍在飞的形态。
#[cfg(feature = "cluster")]
struct SupersededMockProvider {
    plan_id: String,
    content: Option<String>,
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for SupersededMockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        super::RUNNING_PLANS.lock().remove(&self.plan_id);
        match &self.content {
            Some(c) => Ok(nemesis_agent::r#loop::LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: c.clone(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            }),
            None => Err("provider exploded".to_string()),
        }
    }
}

/// 轮询等评论出现（监视任务两跳 spawn，不等固定时长）。`min_count` = 该
/// needle 需出现的最少条数（恢复/重拆场景夹具自带同文首评论，用 2 等新链）。
#[cfg(feature = "cluster")]
async fn wait_for_comment(store: &Arc<BoardStore>, issue_id: i64, needle: &str, min_count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n = store
            .list_comments(issue_id)
            .unwrap()
            .iter()
            .filter(|c| c.content.contains(needle))
            .count();
        if n >= min_count {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "评论未在 5s 内落盘: {needle}（{n}/{min_count}）"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 轮询取事件（类型匹配即返回；空转超时 panic）。
#[cfg(feature = "cluster")]
async fn wait_for_event(
    rx: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
    event_type: &str,
) -> crate::events::Event {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match rx.try_recv() {
            Ok(evt) if evt.event_type == event_type => return evt,
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "事件未在 5s 内到达: {event_type}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => panic!("事件通道异常: {e}"),
        }
    }
}

/// 超时重拆主链路（V2）：短阈值注入 → 监视摘条目 → ⏰ 评论 → attempt+1
/// 新链（plan_started 载荷带 attempt=2）。mock 恒 Err → 新链快速失败收口
///（失败评论 + plan_failed），不影响本断言面。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_timeout_watch_replans_with_next_attempt() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-timeout-replan");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "超时重拆父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider { content: None }),
        nemesis_agent::AgentConfig::default(),
    ));
    let hub = Arc::new(EventHub::new());
    let mut rx = hub.subscribe();
    let plan_id = "pl-to-1".to_string();
    super::RUNNING_PLANS.lock().insert(
        plan_id.clone(),
        super::RunningPlan {
            issue_id: issue.id,
            // checked_sub 而非裸减：Instant 自系统启动起算，CI 新启 VM 纪元余量
            // 不足会下溢 panic（2026-10-18 shard2 实录）。监视超时只由 timeout_secs
            // 睡眠驱动（不读 started_at），回拨不出时用当下时刻语义不变。
            started_at: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1201))
                .unwrap_or_else(std::time::Instant::now),
            attempt: 1,
            phase: "planning",
        },
    );
    let env = Arc::new(super::PlanChainEnv {
        store: store.clone(),
        cluster: None,
        agent_loop,
        issue: issue.clone(),
        actor: nemesis_board::Actor::admin("tester"),
        home: Some(dir.clone()),
        hub: Some(hub.clone()),
    });
    super::spawn_plan_timeout_watch(
        env,
        plan_id.clone(),
        1,
        nemesis_config::BoardPlanConfig {
            timeout_secs: 1,
            max_attempts: 3,
            timeout_reattempt: true,
            restart_recover: true,
            ..Default::default()
        },
    );
    // 旧条目被监视摘除（新链用新 plan_id，本断言只盯旧 id）。
    wait_for_comment(&store, issue.id, "⏰ 拆解超时", 1).await;
    assert!(super::RUNNING_PLANS.lock().get("pl-to-1").is_none());
    let evt = wait_for_event(&mut rx, "board.plan_started").await;
    assert_eq!(evt.data["issue_id"], issue.id);
    assert_eq!(
        evt.data["attempt"], 2,
        "重拆链 plan_started 应带 attempt=2: {:?}",
        evt.data
    );
    assert_ne!(evt.data["plan_id"], "pl-to-1", "重拆链应用新 plan_id");
    // 新链真实发车：新链自己的「已开始自动拆解」评论（本测试未跑首链，
    // 手工只插了注册表条目——首条即重拆链所发）。
    wait_for_comment(&store, issue.id, "已开始自动拆解", 1).await;
    let started_count = store
        .list_comments(issue.id)
        .unwrap()
        .iter()
        .filter(|c| c.content.contains("已开始自动拆解"))
        .count();
    assert_eq!(started_count, 1, "重拆链应发自己的开始评论");
    // 清理：重拆链（默认 cfg）的监视任务不阻塞测试进程，条目兜底清。
    super::RUNNING_PLANS
        .lock()
        .retain(|_, r| r.issue_id != issue.id);
    let _ = std::fs::remove_dir_all(&dir);
}

/// attempt 耗尽停车（V2）：max_attempts=1 时第 1 次超时即停车——停车评论
/// + board.plan_failed 事件 + 无新链（无第二条开始评论）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_timeout_watch_parks_when_attempts_exhausted() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-timeout-park");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "attempt 耗尽停车父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider { content: None }),
        nemesis_agent::AgentConfig::default(),
    ));
    let hub = Arc::new(EventHub::new());
    let mut rx = hub.subscribe();
    let plan_id = "pl-park-1".to_string();
    super::RUNNING_PLANS.lock().insert(
        plan_id.clone(),
        super::RunningPlan {
            issue_id: issue.id,
            started_at: std::time::Instant::now(),
            attempt: 1,
            phase: "planning",
        },
    );
    let env = Arc::new(super::PlanChainEnv {
        store: store.clone(),
        cluster: None,
        agent_loop,
        issue: issue.clone(),
        actor: nemesis_board::Actor::admin("tester"),
        home: Some(dir.clone()),
        hub: Some(hub.clone()),
    });
    super::spawn_plan_timeout_watch(
        env,
        plan_id.clone(),
        1,
        nemesis_config::BoardPlanConfig {
            timeout_secs: 1,
            max_attempts: 1,
            timeout_reattempt: true,
            restart_recover: true,
            ..Default::default()
        },
    );
    wait_for_comment(&store, issue.id, "已连续超时 1 次", 1).await;
    wait_for_comment(&store, issue.id, "请人工处理", 1).await;
    let evt = wait_for_event(&mut rx, "board.plan_failed").await;
    assert_eq!(evt.data["issue_id"], issue.id);
    // 停车 = 无新链：无「已开始自动拆解」评论（本测试没跑过首链）。
    let started = store
        .list_comments(issue.id)
        .unwrap()
        .iter()
        .filter(|c| c.content.contains("已开始自动拆解"))
        .count();
    assert_eq!(started, 0, "停车后不得有新拆解链");
    let _ = std::fs::remove_dir_all(&dir);
}

/// timeout_reattempt=false：超时不重拆直接转人工（开关语义独立于 attempt）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_timeout_watch_parks_when_reattempt_disabled() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-timeout-noretry");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "reattempt 关停车父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider { content: None }),
        nemesis_agent::AgentConfig::default(),
    ));
    let plan_id = "pl-noretry-1".to_string();
    super::RUNNING_PLANS.lock().insert(
        plan_id.clone(),
        super::RunningPlan {
            issue_id: issue.id,
            started_at: std::time::Instant::now(),
            attempt: 1,
            phase: "planning",
        },
    );
    let env = Arc::new(super::PlanChainEnv {
        store: store.clone(),
        cluster: None,
        agent_loop,
        issue: issue.clone(),
        actor: nemesis_board::Actor::admin("tester"),
        home: Some(dir.clone()),
        hub: None,
    });
    super::spawn_plan_timeout_watch(
        env,
        plan_id.clone(),
        1,
        nemesis_config::BoardPlanConfig {
            timeout_secs: 1,
            max_attempts: 3,
            timeout_reattempt: false,
            restart_recover: true,
            ..Default::default()
        },
    );
    wait_for_comment(&store, issue.id, "自动重拆未开启", 1).await;
    let started = store
        .list_comments(issue.id)
        .unwrap()
        .iter()
        .filter(|c| c.content.contains("已开始自动拆解"))
        .count();
    assert_eq!(started, 0, "reattempt 关不得有新拆解链");
    let _ = std::fs::remove_dir_all(&dir);
}

/// timeout_secs=0 = 不限：不挂监视（条目原样保留、零评论零事件）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_timeout_watch_disabled_when_zero() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-timeout-zero");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "不限时父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider { content: None }),
        nemesis_agent::AgentConfig::default(),
    ));
    let plan_id = "pl-zero-1".to_string();
    super::RUNNING_PLANS.lock().insert(
        plan_id.clone(),
        super::RunningPlan {
            issue_id: issue.id,
            started_at: std::time::Instant::now(),
            attempt: 1,
            phase: "planning",
        },
    );
    let env = Arc::new(super::PlanChainEnv {
        store: store.clone(),
        cluster: None,
        agent_loop,
        issue: issue.clone(),
        actor: nemesis_board::Actor::admin("tester"),
        home: Some(dir.clone()),
        hub: None,
    });
    super::spawn_plan_timeout_watch(
        env,
        plan_id.clone(),
        1,
        nemesis_config::BoardPlanConfig {
            timeout_secs: 0,
            ..Default::default()
        },
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        super::RUNNING_PLANS.lock().get("pl-zero-1").is_some(),
        "0=不限：监视不摘条目"
    );
    assert!(store.list_comments(issue.id).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// superseded 双闸·成功路径（V3）：LLM 返回前条目被监视摘除（已被重拆链
/// 取代）→ 静默放弃：status=superseded、不落缓存、无完成评论、无 plan_ready。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_chain_superseded_success_path_is_silent() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-superseded-ok");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "被取代父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let plan_id = "pl-sup-ok".to_string();
    super::RUNNING_PLANS.lock().insert(
        plan_id.clone(),
        super::RunningPlan {
            issue_id: issue.id,
            started_at: std::time::Instant::now(),
            attempt: 1,
            phase: "planning",
        },
    );
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(SupersededMockProvider {
            plan_id: plan_id.clone(),
            content: Some(r#"[{"title":"旧链子单","depends_on":[]}]"#.to_string()),
        }),
        nemesis_agent::AgentConfig::default(),
    ));
    let hub = Arc::new(EventHub::new());
    let mut rx = hub.subscribe();
    let out = super::execute_plan_chain(
        &store,
        None,
        agent_loop,
        issue.clone(),
        nemesis_board::Actor::admin("tester"),
        &plan_id,
        Some(&dir),
        Some(hub.as_ref()),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out["status"], "superseded", "{out}");
    assert!(
        super::PLAN_CACHE.lock().get("pl-sup-ok").is_none(),
        "被取代链不得落预览缓存"
    );
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("planner 拆解完成")),
        "被取代链不得发完成评论: {comments:?}"
    );
    // 事件面：只允许链起点自己的 plan_started（链自身发的），无 plan_ready /
    // plan_failed（旧链迟到事件会撤掉新链横幅——双闸本意）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) if evt.event_type == "board.plan_started" => continue,
            Ok(evt) => panic!("被取代链不得再发事件: {}", evt.event_type),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(e) => panic!("事件通道异常: {e}"),
        }
    }
    super::RUNNING_PLANS.lock().remove(&plan_id);
    let _ = std::fs::remove_dir_all(&dir);
}

/// superseded 双闸·失败路径（V3）：旧链 planner 失败且已被取代 → 返回
/// Ok(superseded)（不外抛错误），无失败评论、无 plan_failed（旧链迟到的
/// 失败事件会撤掉新链横幅——双闸的本意）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn plan_chain_superseded_failure_path_is_silent() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-superseded-err");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "被取代且失败父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    let plan_id = "pl-sup-err".to_string();
    super::RUNNING_PLANS.lock().insert(
        plan_id.clone(),
        super::RunningPlan {
            issue_id: issue.id,
            started_at: std::time::Instant::now(),
            attempt: 1,
            phase: "planning",
        },
    );
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(SupersededMockProvider {
            plan_id: plan_id.clone(),
            content: None,
        }),
        nemesis_agent::AgentConfig::default(),
    ));
    let hub = Arc::new(EventHub::new());
    let mut rx = hub.subscribe();
    let out = super::execute_plan_chain(
        &store,
        None,
        agent_loop,
        issue.clone(),
        nemesis_board::Actor::admin("tester"),
        &plan_id,
        Some(&dir),
        Some(hub.as_ref()),
        1,
    )
    .await
    .unwrap();
    assert_eq!(out["status"], "superseded", "{out}");
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("planner 拆解失败")),
        "被取代链不得发失败评论: {comments:?}"
    );
    // 事件面：只允许链起点自己的 plan_started，无 plan_failed。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) if evt.event_type == "board.plan_started" => continue,
            Ok(evt) => panic!("被取代链不得再发事件: {}", evt.event_type),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(e) => panic!("事件通道异常: {e}"),
        }
    }
    super::RUNNING_PLANS.lock().remove(&plan_id);
    let _ = std::fs::remove_dir_all(&dir);
}

/// issue.list planning 投影（§1.1 A）：命中 RUNNING_PLANS 的单带
/// planning{elapsed_secs, attempt}（重拆后 attempt 正确）；未命中单无该键。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_list_includes_planning_projection() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("issue-list-planning");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let a = store
        .create_issue(nemesis_board::NewIssue {
            title: "拆解中甲".to_string(),
            ..Default::default()
        })
        .unwrap();
    let b = store
        .create_issue(nemesis_board::NewIssue {
            title: "旁观的乙".to_string(),
            ..Default::default()
        })
        .unwrap();
    // 65s 回拨需 Instant 纪元余量（elapsed_secs>=64 断言依赖真实历史时刻）；
    // 刚启动系统回拨不了 → 环境边界诚实跳过（同过期态断言纪律）。
    let Some(started) = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(65))
    else {
        eprintln!("SKIP planning 投影断言：系统刚启动，Instant 纪元余量不足 65s");
        return;
    };
    super::RUNNING_PLANS.lock().insert(
        "pl-proj-1".to_string(),
        super::RunningPlan {
            issue_id: a.id,
            started_at: started,
            attempt: 2,
            phase: "planning",
        },
    );
    let out = dispatch(&ctx, "issue.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let rows = out["issues"].as_array().unwrap();
    let ra = rows.iter().find(|r| r["id"] == a.id).unwrap();
    let rb = rows.iter().find(|r| r["id"] == b.id).unwrap();
    assert!(ra["planning"].is_object(), "命中单应带 planning 投影: {ra}");
    assert_eq!(ra["planning"]["attempt"], 2, "重拆后 attempt 应正确");
    assert!(
        ra["planning"]["elapsed_secs"].as_u64().unwrap() >= 64,
        "elapsed 应为已等待秒数: {ra}"
    );
    assert!(
        rb.get("planning").is_none(),
        "未命中单不得带 planning 键: {rb}"
    );
    super::RUNNING_PLANS.lock().remove("pl-proj-1");
    let _ = std::fs::remove_dir_all(&dir);
}

/// N1 重启恢复（V4 单测臂）：中断形态命中（backlog/todo + 最新系统评论=
/// 拆解中 + 无子单 + 不在 RUNNING_PLANS）→ 自动重拆；守卫臂：开关关 /
/// estop 挂起 / 有子单 / 最新系统评论已被顶掉 / 非 backlog 状态均不恢复。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn recover_interrupted_plans_scans_and_respects_guards() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    let dir = unique_dir("plan-recover-n1");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(PlanChainMockProvider { content: None }),
        nemesis_agent::AgentConfig::default(),
    ));
    let hub = Arc::new(EventHub::new());
    let started_comment = |id: i64| nemesis_board::models::NewComment {
        issue_id: id,
        author: nemesis_board::Actor::system("board"),
        content: "🧩 已开始自动拆解（planner 运行中，通常 3~8 分钟）".to_string(),
        parent_id: None,
        ctype: nemesis_board::models::CommentType::System,
    };
    // 命中形态。
    let hit = store
        .create_issue(nemesis_board::NewIssue {
            title: "中断残留父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    store.add_comment(started_comment(hit.id)).unwrap();
    // 守卫臂 1：最新系统评论被失败评论顶掉（非中断形态）。
    let done = store
        .create_issue(nemesis_board::NewIssue {
            title: "已收口父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    store.add_comment(started_comment(done.id)).unwrap();
    store
        .add_comment(nemesis_board::models::NewComment {
            issue_id: done.id,
            author: nemesis_board::Actor::system("board"),
            content: "❌ planner 拆解失败：provider exploded".to_string(),
            parent_id: None,
            ctype: nemesis_board::models::CommentType::System,
        })
        .unwrap();
    // 守卫臂 2：已有子单（拆解实际发过车）→ 让位。
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "有子单父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    store.add_comment(started_comment(parent.id)).unwrap();
    store
        .create_issue(nemesis_board::NewIssue {
            title: "历史子单".to_string(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    // 守卫臂 3：非 backlog 状态（in_progress 天然不在扫描集）。
    let active = store
        .create_issue(nemesis_board::NewIssue {
            title: "进行中父单".to_string(),
            ..Default::default()
        })
        .unwrap();
    store.add_comment(started_comment(active.id)).unwrap();
    store
        .transition_issue(
            active.id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::admin("tester"),
        )
        .unwrap();

    // 守卫臂 4：estop 挂起 → 全跳过（急停意志优先）。
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    assert_eq!(
        super::recover_interrupted_plans(
            &store,
            None,
            agent_loop.clone(),
            Some(dir.clone()),
            Some(hub.clone()),
            Some(&estop)
        ),
        0,
        "estop 挂起不得恢复拆解"
    );
    estop.release();

    // 守卫臂 5：开关关（config.json board.plan.restart_recover=false）→ 0。
    std::fs::write(
        dir.join("config.json"),
        serde_json::json!({"board": {"plan": {"restart_recover": false}}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        super::recover_interrupted_plans(
            &store,
            None,
            agent_loop.clone(),
            Some(dir.clone()),
            Some(hub.clone()),
            None
        ),
        0,
        "restart_recover=false 不得恢复拆解"
    );
    std::fs::remove_file(dir.join("config.json")).unwrap();

    // 主臂：恢复命中形态 1 个（done/parent/active 三守卫臂被排除）。
    let recovered = super::recover_interrupted_plans(
        &store,
        None,
        agent_loop,
        Some(dir.clone()),
        Some(hub.clone()),
        None,
    );
    assert_eq!(recovered, 1, "只有中断残留父单应被恢复");
    wait_for_comment(&store, hit.id, "检测到上次拆解因重启中断", 1).await;
    // 恢复链真实发车：恢复链自己的「已开始自动拆解」（夹具首评论同文——
    // 等第 2 条，防 wait 被夹具立即满足后 count 跑在 spawn 落盘前）。
    wait_for_comment(&store, hit.id, "已开始自动拆解", 2).await;
    let started_count = store
        .list_comments(hit.id)
        .unwrap()
        .iter()
        .filter(|c| c.content.contains("已开始自动拆解"))
        .count();
    assert_eq!(started_count, 2, "恢复应发起新拆解链");
    // 幂等：恢复后单子已在 RUNNING_PLANS（新链在途）——再扫不重复恢复。
    let again = super::recover_interrupted_plans(
        &store,
        None,
        Arc::new(nemesis_agent::r#loop::AgentLoop::new(
            Box::new(PlanChainMockProvider { content: None }),
            nemesis_agent::AgentConfig::default(),
        )),
        Some(dir.clone()),
        Some(hub.clone()),
        None,
    );
    assert_eq!(again, 0, "在途链让位：不得重复恢复");
    // 清理：恢复链（默认 cfg 1200s 监视）条目兜底清。
    super::RUNNING_PLANS
        .lock()
        .retain(|_, r| r.issue_id != hit.id);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- U-C（2026-10-12 §9.3）：issue.timeline 表面契约 ---

#[tokio::test]
async fn test_issue_timeline_surface_contract() {
    let dir = unique_dir("timeline");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();

    let id = store
        .create_issue(NewIssue {
            title: "时间线单".into(),
            creator: Actor::admin("tester"),
            ..NewIssue::default()
        })
        .unwrap()
        .id;
    store
        .add_activity(
            id,
            &Actor::admin("tester"),
            "auto_decide",
            Some(r#"{"verdict":"PASS"}"#),
        )
        .unwrap();
    store
        .add_comment(NewComment {
            issue_id: id,
            author: Actor::admin("tester"),
            content: "交付首行\n全文第二行".into(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();
    store
        .insert_dispatch("task-tl", id, "node-w", &Actor::admin("tester"))
        .unwrap();

    // by id：事件按时间升序，源齐备；无 cluster 装配 → 无 cluster_task/continuation。
    let out = dispatch(&ctx, "issue.timeline", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"], "NB-1");
    let events = out["events"].as_array().expect("events array");
    let sources: Vec<&str> = events
        .iter()
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert!(sources.contains(&"activity"), "sources: {sources:?}");
    assert!(sources.contains(&"comment") && sources.contains(&"dispatch"));
    assert!(!sources.contains(&"cluster_task") && !sources.contains(&"continuation"));
    // 时间升序（同刻 tie-break 也不允许倒序）。
    let tss: Vec<i64> = events.iter().map(|e| e["ts"].as_i64().unwrap()).collect();
    let mut sorted = tss.clone();
    sorted.sort_unstable();
    assert_eq!(tss, sorted, "events not time-ascending: {tss:?}");
    // 评论摘要 = 首行。
    let comment = events.iter().find(|e| e["source"] == "comment").unwrap();
    assert_eq!(comment["summary"], "交付首行");

    // by number 兼容。
    let out = dispatch(
        &ctx,
        "issue.timeline",
        serde_json::json!({ "number": "NB-1" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue_id"], id);

    // 缺参 → 诚实报错；单子不存在 → 诚实报错。
    assert!(
        dispatch(&ctx, "issue.timeline", serde_json::json!({}))
            .await
            .is_err()
    );
    assert!(
        dispatch(
            &ctx,
            "issue.timeline",
            serde_json::json!({ "number": "NB-999" })
        )
        .await
        .is_err()
    );
    // commands() 清单含 issue.timeline（L1 注册表漂移闸）。
    assert!(BoardHandler.commands().contains(&"issue.timeline"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// U-A（2026-10-12 停车现场工程）：no_match 停车落现场档案。HANDOFF_
/// WORKSPACE 是模块级 OnceLock（BASELINE_PUSHER 同款单测不装配零波及）——
/// 本测试是全进程唯一装配点，装一次性临时目录后断言 handoffs/<NB-x>.json。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn ua_no_match_park_writes_handoff_archive() {
    // 派发路径读 PLAN_CACHE/RUNNING_PLANS 进程级全局（BUG-② 派发闸），
    // 与 seed_plan/plan 链测试按 issue id 撞库——串行持锁 + 开场清残留。
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("ua-handoff-ws");
    let ws = dir.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    assert!(
        super::install_handoff_workspace(ws.clone()),
        "本测试是 HANDOFF_WORKSPACE 的唯一装配点（首装恒成功）"
    );

    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-ua");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "UA 父".into(),
            ..Default::default()
        })
        .unwrap();
    let a = planner_child(&store, parent.id, "UA 子A", vec!["no-such-tag".to_string()]);

    // 无匹配停车（⏸ 评论照常）+ 档案落盘。
    assert_eq!(
        super::dispatch_subissue_auto(&store, Some(&cluster), a.id, &actor, true).unwrap(),
        None
    );
    let path = nemesis_board::handoff::handoff_path(&ws, &a.number);
    assert!(path.is_file(), "停车档案必须落盘：{}", path.display());
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["stop_kind"], "no_match");
    assert_eq!(doc["suggested_action"], "retry");
    assert_eq!(
        doc["environment"]["required_tags"],
        serde_json::json!(["no-such-tag"])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------
// F2 送达失败自动重试 sweep 闸序（多角度审计批 2026-10-13）：预算熔断 →
// 退避 → 执行器。真派发腿（RPC 达成）归 cluster-uat；本节钉 surface 契约。
// ------------------------------------------------------------------

/// 造一个「送达失败池成员」：backlog + 失败派发行 + n 条 ⛔ 系统评论。
#[cfg(feature = "cluster")]
fn send_failure_issue(
    store: &Arc<BoardStore>,
    actor: &Actor,
    title: &str,
    markers: usize,
) -> nemesis_board::Issue {
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: title.to_string(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch(
            &format!("task-sf-{}", issue.id),
            issue.id,
            "node-gone",
            actor,
        )
        .unwrap();
    store
        .finish_dispatch(&format!("task-sf-{}", issue.id), "failed")
        .unwrap();
    for i in 0..markers {
        store
            .add_comment(nemesis_board::models::NewComment {
                issue_id: issue.id,
                author: actor.clone(),
                content: format!(
                    "{}：[送达] 第 {i} 次连接拒绝",
                    nemesis_board::SEND_FAILURE_MARK
                ),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            })
            .unwrap();
    }
    issue
}

/// 预算熔断：attempts(2) > cap(1) → 🛑 转人工评论，落盘即出池。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn send_failure_budget_fuse_parks_issue_with_stop_comment() {
    let dir = unique_dir("sf-fuse");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let issue = send_failure_issue(&store, &actor, "超预算单", 2);
    let pool = || {
        store
            .list_send_failure_candidates(nemesis_board::SEND_FAILURE_MARK)
            .unwrap()
    };
    assert_eq!(pool().len(), 1, "前置：失败单在池");

    let cfg = nemesis_config::BoardFlagConfig {
        budget: nemesis_config::BoardBudgetConfig {
            max_send_retries: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let cluster = offline_cluster(&dir, "coord-sf-fuse");
    let (considered, dispatched, _failed) =
        super::sweep_parked_dispatches_with_config(Some(&cfg), &store, &cluster, &actor, false);
    assert_eq!(dispatched, 0, "熔断不得派发");
    assert_eq!(considered, 1, "池内唯一成员被审视");
    let comments = store.list_comments(issue.id).unwrap();
    let stop = comments
        .iter()
        .find(|c| c.content.contains("已达预算上限"))
        .expect("🛑 转人工评论必须落盘");
    assert!(
        stop.content.contains("max_send_retries=1"),
        "文案带配置键：{}",
        stop.content
    );
    assert!(
        pool().is_empty(),
        "🛑 落盘 = 最后一条 system 评论不再是 ⛔ → 出池"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// unlimited_mode=true → 熔断关闭（cap 视为 0）：不派发、不落评论、
/// 单子留在池里等退避。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn send_failure_fuse_disabled_by_unlimited_mode() {
    let dir = unique_dir("sf-nofuse");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let issue = send_failure_issue(&store, &actor, "不限模式单", 2);
    let cfg = nemesis_config::BoardFlagConfig {
        unlimited_mode: true,
        budget: nemesis_config::BoardBudgetConfig {
            max_send_retries: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    let cluster = offline_cluster(&dir, "coord-sf-free");
    let (_, dispatched, _) =
        super::sweep_parked_dispatches_with_config(Some(&cfg), &store, &cluster, &actor, false);
    assert_eq!(dispatched, 0, "退避未到（fresh ⛔）不得派发");
    assert!(
        !store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("已达预算上限")),
        "unlimited_mode 下熔断必须关闭"
    );
    assert_eq!(
        store
            .list_send_failure_candidates(nemesis_board::SEND_FAILURE_MARK)
            .unwrap()
            .len(),
        1,
        "仍在池等退避"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 退避闸：末次失败 = 刚刚（attempts=1 → 60s 窗口），sweep 本 tick 必须跳过
/// ——零派发、零副作用、留池。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn send_failure_backoff_gate_skips_fresh_failure() {
    let dir = unique_dir("sf-backoff");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let issue = send_failure_issue(&store, &actor, "退避中单", 1);
    let cluster = offline_cluster(&dir, "coord-sf-bo");
    let (considered, dispatched, failed) =
        super::sweep_parked_dispatches_with_config(None, &store, &cluster, &actor, false);
    assert_eq!(considered, 1);
    assert_eq!(dispatched, 0, "退避窗口内不得派发");
    assert_eq!(failed, 0);
    assert_eq!(
        store.list_comments(issue.id).unwrap().len(),
        1,
        "退避跳过零副作用（只有那条 ⛔）"
    );
    assert_eq!(
        store
            .list_send_failure_candidates(nemesis_board::SEND_FAILURE_MARK)
            .unwrap()
            .len(),
        1,
        "留池"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// recover_review_feedback：只认「❌ agent 验收未通过」**开头**的 Comment；
/// 人工评论引用「## 差距」字样（contains 曾误捞）不得误当自动重派差距。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn recover_review_feedback_anchored_prefix_not_contains() {
    let dir = unique_dir("sf-feedback");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "反馈找回".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        super::recover_review_feedback(&store, issue.id),
        None,
        "无评论不造空意见"
    );

    let user_comment = |content: &str| {
        store
            .add_comment(nemesis_board::models::NewComment {
                issue_id: issue.id,
                author: actor.clone(),
                content: content.to_string(),
                parent_id: None,
                ctype: nemesis_board::CommentType::Comment,
            })
            .unwrap();
    };
    // 人工评审引用「## 差距」字样（非 ❌ 开头）——contains 语义会误捞。
    user_comment("worker 自述里写了 ## 差距 这几个字，但我这条是人工意见");
    assert_eq!(
        super::recover_review_feedback(&store, issue.id),
        None,
        "引用字样的人工评论不得误捞"
    );
    // 自动重派差距评论（❌ 开头）——唯一合法来源。
    user_comment("❌ agent 验收未通过（第 1/2 次重派）## 差距\n- 缺测试");
    let fb = super::recover_review_feedback(&store, issue.id).expect("❌ 开头必须捞起");
    assert!(fb.starts_with("❌ agent 验收未通过"), "整段原文：{fb}");
    assert!(fb.contains("缺测试"));
    // 更新的一条覆盖旧条（取最后）。
    user_comment("❌ agent 验收未通过（第 2/2 次重派）## 差距\n- 仍缺测试");
    assert!(
        super::recover_review_feedback(&store, issue.id)
            .unwrap()
            .contains("仍缺测试"),
        "取最后一条 ❌ 开头评论"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// pick_send_failure_redispatch_target 三臂：末次 worker 在线 → 原地；
/// 末次 worker 消失 → 在线候选榜首；无派发记录 → None。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn pick_send_failure_target_prefers_last_worker_then_best_online() {
    let dir = unique_dir("sf-target");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-sf-t");
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "node-py".into(),
        name: "PyWorker".into(),
        address: "127.0.0.1:19999".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["python".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 无派发记录的单：在线候选存在时会走 rank 榜首臂；None 臂在下方
    // 空注册表 cluster 上断言（全员离线 = 调用方留池的语义面）。
    let bare = store
        .create_issue(nemesis_board::NewIssue {
            title: "无派发".into(),
            ..Default::default()
        })
        .unwrap();

    // 末次 worker 在线 → 原地重试。
    let online = store
        .create_issue(nemesis_board::NewIssue {
            title: "末次在线".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("task-t1", online.id, "node-py", &actor)
        .unwrap();
    assert_eq!(
        super::pick_send_failure_redispatch_target(&store, &cluster, online.id).as_deref(),
        Some("node-py"),
        "末次 worker 在线 = 原地重试"
    );

    // 末次 worker 消失（从未注册）→ 在线候选榜首（node-py 唯一在线）。
    let gone = store
        .create_issue(nemesis_board::NewIssue {
            title: "末次消失".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("task-t2", gone.id, "node-vanished", &actor)
        .unwrap();
    assert_eq!(
        super::pick_send_failure_redispatch_target(&store, &cluster, gone.id).as_deref(),
        Some("node-py"),
        "末次 worker 消失 → 在线候选首选"
    );

    // 全员离线（换空注册表的 cluster）→ None。
    let empty_cluster = offline_cluster(&dir, "coord-sf-empty");
    empty_cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    assert_eq!(
        super::pick_send_failure_redispatch_target(&store, &empty_cluster, bare.id),
        None,
        "无派发记录 + 无在线节点 → None（调用方留池）"
    );
    assert_eq!(
        super::pick_send_failure_redispatch_target(&store, &empty_cluster, gone.id),
        None,
        "末次 worker 消失 + 无在线节点 → None"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// BUG 2026-10-08 `board.plan.model` dead-key 修复回归：覆盖解析 helper 的
/// 三臂（解析成功 / 未配置·空别名回落 None / 未知别名回落 None——不
/// fail-closed，warn 留痕）。
#[cfg(feature = "cluster")]
#[test]
fn board_plan_model_swap_resolution_arms() {
    // ① home 无 config.json → None（无覆盖 = 主模型，安静回落）。
    let dir = unique_dir("plan-swap-empty");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(super::board_plan_model_swap(Some(&dir)).is_none());

    // ② 合法别名 → Some，模型名是解析后的裸名（provider 前缀剥掉）。
    let dir = unique_dir("plan-swap-ok");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{
            "model_list": [
                {"model_name": "planner-model", "model": "openai/planner-x", "api_key": "k"}
            ],
            "board": {"plan": {"model": "planner-model"}}
        }"#,
    )
    .unwrap();
    let swap = super::board_plan_model_swap(Some(&dir)).expect("alias resolves");
    assert_eq!(swap.1, "planner-x");

    // ③ 显式空串 = 显式不覆盖 → None。
    let dir = unique_dir("plan-swap-blank");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{"board": {"plan": {"model": ""}}}"#,
    )
    .unwrap();
    assert!(super::board_plan_model_swap(Some(&dir)).is_none());

    // ④ 未知别名 → None（warn + 回落主模型，不 panic 不 fail-closed）。
    let dir = unique_dir("plan-swap-unknown");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{"board": {"plan": {"model": "no-such-alias"}}}"#,
    )
    .unwrap();
    assert!(super::board_plan_model_swap(Some(&dir)).is_none());

    for d in [
        unique_dir("plan-swap-empty"),
        unique_dir("plan-swap-ok"),
        unique_dir("plan-swap-blank"),
        unique_dir("plan-swap-unknown"),
    ] {
        let _ = std::fs::remove_dir_all(&d);
    }
}

// 2026-10-15 依赖徽标投影：issue.list / issue.get 都带 depends_on
// [{id, number, status}]；status 变化后投影跟随（前端阻塞态判断数据源）。
#[tokio::test]
async fn test_issue_depends_on_projection() {
    let dir = unique_dir("depends-on-projection");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();

    // 两个单：NB-1 上游、NB-2 依赖 NB-1。
    let a = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "上游" }))
        .await
        .unwrap()
        .unwrap()["issue"]["id"]
        .as_i64()
        .unwrap();
    let b = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "下游" }))
        .await
        .unwrap()
        .unwrap()["issue"]["id"]
        .as_i64()
        .unwrap();
    store.set_issue_dependencies(b, &[a]).unwrap();

    // list：下游行带 depends_on，上游行空数组（加法投影，无依赖也输出键）。
    let out = dispatch(&ctx, "issue.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let rows = out["issues"].as_array().unwrap();
    let down = rows.iter().find(|r| r["id"] == b).unwrap();
    let ups = down["depends_on"].as_array().unwrap();
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0]["id"], a);
    assert_eq!(ups[0]["number"], "NB-1");
    assert_eq!(ups[0]["status"], "backlog");
    let up = rows.iter().find(|r| r["id"] == a).unwrap();
    assert!(up["depends_on"].as_array().unwrap().is_empty());

    // get：同投影存在。
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "id": b }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["depends_on"][0]["number"], "NB-1");

    // 上游 done 后投影跟随（前端据此解除阻塞徽标）。
    store
        .transition_issue(
            a,
            nemesis_board::models::IssueStatus::Done,
            &nemesis_board::Actor::system("board"),
        )
        .unwrap();
    let out = dispatch(&ctx, "issue.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let rows = out["issues"].as_array().unwrap();
    let down = rows.iter().find(|r| r["id"] == b).unwrap();
    assert_eq!(down["depends_on"][0]["status"], "done");

    let _ = std::fs::remove_dir_all(&dir);
}
// ---------------------------------------------------------------------------
// 决策可见性投影（2026-10-15 批）：needs_user（停车强信号）/ attention
// （停滞弱信号）/ plan_ready（计划待发车中信号）三投影 + issue.decision
// 面板数据源。HANDOFF_WORKSPACE 是进程级 OnceLock（ua 测试唯一装配）——
// 全局侧走 scan_live_handoffs_in 纯内核，装配路径由 dispatch 级「无信号
// 形态」钉住（未装配 = 空档案集，诚实降级）。
// ---------------------------------------------------------------------------

fn handoff_map_entry(number: &str, stop_kind: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "issue": number,
        "issue_id": 42,
        "stop_kind": stop_kind,
        "suggested_action": "manual_check",
        "hint": "先修 worker 环境再复评",
        "stopped_at": "2026-10-15T07:38:00Z",
    })
}

/// 状态闸：活档案只对 in_review / in_progress 亮牌——done/cancelled/backlog
/// 即使有漏归档旧档案也不得点亮（正确性兜底，归档只是时效优化）。
#[test]
fn needs_user_projection_gates_by_status() {
    let mut handoffs = std::collections::HashMap::new();
    handoffs.insert(
        "NB-5".to_string(),
        handoff_map_entry("NB-5", "unsure_streak"),
    );

    // 底座单走 store 构造（Issue 无 Default）；测试只消费 status/number，
    // 其余字段直接在 owned 副本上变异。
    let dir = unique_dir("needs-user-gate");
    let store = BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let base = store
        .create_issue(nemesis_board::NewIssue {
            title: "闸门测试".into(),
            ..Default::default()
        })
        .unwrap();
    let mk = |status| nemesis_board::Issue {
        status,
        number: "NB-5".to_string(),
        ..base.clone()
    };

    // 闸内两态：亮牌 + 字段形态（stop_kind_label 映射、hint/stopped_at 透传）。
    let nu = super::needs_user_projection(&handoffs, &mk(IssueStatus::InReview)).unwrap();
    assert_eq!(nu["stop_kind"], "unsure_streak");
    assert_eq!(nu["stop_kind_label"], "连续不确定");
    assert_eq!(nu["suggested_action"], "manual_check");
    assert_eq!(nu["hint"], "先修 worker 环境再复评");
    assert_eq!(nu["stopped_at"], "2026-10-15T07:38:00Z");
    assert!(super::needs_user_projection(&handoffs, &mk(IssueStatus::InProgress)).is_some());

    // 闸外：有档案也不亮。
    for status in [
        IssueStatus::Backlog,
        IssueStatus::Todo,
        IssueStatus::Done,
        IssueStatus::Blocked,
        IssueStatus::Cancelled,
    ] {
        assert!(
            super::needs_user_projection(&handoffs, &mk(status)).is_none(),
            "status={status:?} 不得亮牌"
        );
    }

    // 无档案：闸内也不亮。
    let empty: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    assert!(super::needs_user_projection(&empty, &mk(IssueStatus::InReview)).is_none());

    // 未知 stop_kind：标签兜底「停车待处理」，不炸投影。
    let mut odd = std::collections::HashMap::new();
    odd.insert("NB-5".to_string(), handoff_map_entry("NB-5", "future_kind"));
    let nu = super::needs_user_projection(&odd, &mk(IssueStatus::InReview)).unwrap();
    assert_eq!(nu["stop_kind_label"], "停车待处理");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 可测内核扫描：合法档案进 map（按 issue 字段建键）；坏 JSON / 非 json
/// 后缀 / archive/ 子目录 / 缺 issue 字段 / 目录缺席 = 跳过（诚实无信号）。
#[test]
fn scan_live_handoffs_in_scans_valid_and_skips_dirty() {
    let dir = unique_dir("scan-handoffs");
    let handoffs = dir.join("board").join("handoffs");
    std::fs::create_dir_all(&handoffs).unwrap();
    std::fs::write(
        handoffs.join("NB-5.json"),
        serde_json::to_string(&handoff_map_entry("NB-5", "no_match")).unwrap(),
    )
    .unwrap();
    std::fs::write(
        handoffs.join("NB-6.json"),
        serde_json::to_string(&handoff_map_entry("NB-6", "budget")).unwrap(),
    )
    .unwrap();
    std::fs::write(handoffs.join("broken.json"), "{not json").unwrap();
    std::fs::write(handoffs.join("note.txt"), "x").unwrap();
    std::fs::create_dir_all(handoffs.join("archive")).unwrap();
    std::fs::write(
        handoffs.join("archive").join("NB-7.json"),
        serde_json::to_string(&handoff_map_entry("NB-7", "budget")).unwrap(),
    )
    .unwrap();
    std::fs::write(handoffs.join("NB-8.json"), r#"{"schema":1}"#).unwrap(); // 缺 issue 字段

    let map = super::scan_live_handoffs_in(&dir);
    assert_eq!(map.len(), 2, "只有 NB-5/NB-6 入册：{:?}", map.keys());
    assert_eq!(map["NB-5"]["stop_kind"], "no_match");
    assert_eq!(map["NB-6"]["stop_kind"], "budget");

    // 目录缺席 → 空 map（未装配/未停过车的常态路径）。
    let empty_dir = unique_dir("scan-handoffs-empty");
    assert!(super::scan_live_handoffs_in(&empty_dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty_dir);
}

/// 停滞弱信号：in_progress 且最后一条 system 评论命中 STALL_MARKER → true；
/// 普通 system 评论 / 非 in_progress / 无评论 = false。
#[cfg(feature = "cluster")]
#[test]
fn issue_needs_attention_gates_by_stall_marker_and_status() {
    let dir = unique_dir("attention");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("t");
    let mk = |title: &str| {
        store
            .create_issue(nemesis_board::NewIssue {
                title: title.to_string(),
                ..Default::default()
            })
            .unwrap()
    };

    let stalled = mk("停摆单");
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: stalled.id,
            author: actor.clone(),
            content: format!("{}：交付已收 900s 仍未合入。", nemesis_board::STALL_MARKER),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    store
        .transition_issue(stalled.id, IssueStatus::InProgress, &actor)
        .unwrap();
    let stalled = store.get_issue(stalled.id).unwrap();
    assert!(super::issue_needs_attention(&store, &stalled));

    // 最新 system 评论无标记（停摆后被新一轮派发评论覆盖）→ 熄灭。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: stalled.id,
            author: actor.clone(),
            content: "正常系统评论".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    let stalled = store.get_issue(stalled.id).unwrap();
    assert!(!super::issue_needs_attention(&store, &stalled));

    // 状态闸：in_review 带 STALL_MARKER 也不算（那是 needs_user 的领域）。
    let review = mk("送审单");
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: review.id,
            author: actor,
            content: format!("{}：x", nemesis_board::STALL_MARKER),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    let mut review = store.get_issue(review.id).unwrap();
    review.status = IssueStatus::InReview;
    assert!(!super::issue_needs_attention(&store, &review));

    // 无评论：false。
    let quiet = mk("安静单");
    assert!(!super::issue_needs_attention(&store, &quiet));
    let _ = std::fs::remove_dir_all(&dir);
}

/// dispatch 级接线（无信号形态 + plan_ready 种入形态）：HANDOFF_WORKSPACE
/// 未装配 = needs_user 恒缺席（诚实降级，不炸列表）；plan_ready 走
/// PLAN_CACHE 全局——按 PLAN_TEST_LOCK 纪律串行 + 开场清残留。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn decision_visibility_projections_via_list_and_decision() {
    let _serial = PLAN_TEST_LOCK.lock().await;
    clear_plan_globals();
    let dir = unique_dir("decision-visibility");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("t");

    // 三单：parked(in_review 停车形态——无档案故 needs_user 不亮，钉诚实
    // 降级)、stalled(in_progress + 停摆哨兵)、plain(backlog 无事)。
    let parked = store
        .create_issue(nemesis_board::NewIssue {
            title: "停车单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(parked.id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(parked.id, IssueStatus::InReview, &actor)
        .unwrap();

    let stalled = store
        .create_issue(nemesis_board::NewIssue {
            title: "停滞单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: stalled.id,
            author: actor.clone(),
            content: format!("{}：交付已收 900s 仍未合入。", nemesis_board::STALL_MARKER),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    store
        .transition_issue(stalled.id, IssueStatus::InProgress, &actor)
        .unwrap();

    let plain = store
        .create_issue(nemesis_board::NewIssue {
            title: "安静单".into(),
            ..Default::default()
        })
        .unwrap();

    // issue.list：attention 只落在停滞单；needs_user 无档案恒缺席。
    let out = dispatch(&ctx, "issue.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let rows = out["issues"].as_array().unwrap().clone();
    let row = |num: &str| {
        rows.iter()
            .find(|r| r["number"] == num)
            .unwrap_or_else(|| panic!("row {num} missing"))
            .clone()
    };
    assert_eq!(row(&parked.number)["attention"], serde_json::Value::Null);
    assert_eq!(row(&stalled.number)["attention"], serde_json::json!(true));
    assert_eq!(row(&plain.number)["attention"], serde_json::Value::Null);
    assert_eq!(
        row(&parked.number)["needs_user"],
        serde_json::Value::Null,
        "未装配档案根 = 无信号，不得假亮"
    );
    assert_eq!(row(&parked.number)["plan_ready"], serde_json::Value::Null);

    // plan_ready 投影：缓存种入 → list 与 decision 同构带出。
    seed_plan("plan-dv-1", parked.id, vec![sub("子单", vec![])]);
    let out = dispatch(&ctx, "issue.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let rows = out["issues"].as_array().unwrap();
    let pr = rows.iter().find(|r| r["number"] == parked.number).unwrap()["plan_ready"].clone();
    assert_eq!(pr["plan_id"], "plan-dv-1");
    assert_eq!(pr["subs_count"], 1);
    assert!(pr["created_age_secs"].is_u64());

    // issue.decision：无参/未知单诚实报错。
    assert!(
        dispatch(&ctx, "issue.decision", serde_json::json!({}))
            .await
            .is_err()
    );
    assert!(
        dispatch(
            &ctx,
            "issue.decision",
            serde_json::json!({ "number": "NB-999" })
        )
        .await
        .is_err()
    );

    // decision 面板形态：needs_user/plan_ready/attention/handoff 全字段在
    // 场（online_nodes 无集群装配 = 空数组）；档案缺席 = handoff null。
    let out = dispatch(
        &ctx,
        "issue.decision",
        serde_json::json!({ "number": parked.number }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["issue"], parked.number);
    assert_eq!(out["issue_id"], parked.id);
    assert_eq!(out["status"], "in_review");
    assert_eq!(out["needs_user"], serde_json::Value::Null);
    assert_eq!(out["handoff"], serde_json::Value::Null);
    assert_eq!(out["plan_ready"]["plan_id"], "plan-dv-1");
    assert_eq!(out["attention"], false);
    assert_eq!(out["online_nodes"], serde_json::json!([]));

    // 停滞单走 decision：attention true。
    let out = dispatch(
        &ctx,
        "issue.decision",
        serde_json::json!({ "id": stalled.id }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["status"], "in_progress");
    assert_eq!(out["attention"], true);
    assert_eq!(out["plan_ready"], serde_json::Value::Null);

    // commands() 清单含 issue.decision（L1 注册表漂移闸）。
    assert!(BoardHandler.commands().contains(&"issue.decision"));

    // 清残留（plan 链纪律：持锁期间摘自己的条目）。
    super::PLAN_CACHE.lock().remove("plan-dv-1");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- board.resume（停车续做正门，2026-10-16 主会话污染根修） ---

#[test]
fn test_board_resume_prompt_contract() {
    let archive = r#"{"issue":"NB-9","reason":"UNSURE"}"#;
    let p = super::build_resume_prompt(
        "NB-9",
        "标题甲",
        "in_review",
        archive,
        Some("先复核锚点证据"),
    );
    assert!(p.contains("# 停车续做任务：NB-9"));
    assert!(p.contains("标题：标题甲"));
    assert!(p.contains("当前状态：in_review"));
    assert!(p.contains(archive), "档案 JSON 原文必须整体内联");
    assert!(p.contains("handoffs/NB-9.json"));
    assert!(p.contains("board_issue"), "必须点名 board_issue 工具落评论");
    assert!(p.contains("## 操作员附加指令\n先复核锚点证据"));

    // 无附加指令 / 纯空白指令 → 不出现操作员段（不造空段落）。
    let p2 = super::build_resume_prompt("NB-9", "标题甲", "in_review", archive, None);
    assert!(!p2.contains("操作员附加指令"));
    let p3 = super::build_resume_prompt("NB-9", "标题甲", "in_review", archive, Some("  \n "));
    assert!(!p3.contains("操作员附加指令"));
}

#[test]
fn test_board_resume_archive_path_shape() {
    let p = super::handoff_archive_path("C:/ws", "NB-12");
    let s = p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
    assert!(s.ends_with("board/handoffs/NB-12.json"), "actual: {s}");
}

#[tokio::test]
async fn test_board_resume_surface_contract() {
    let dir = unique_dir("board-resume");
    let ctx = make_ctx_with_board(&dir);

    // 测试 ctx 无 agent_loop：装配性错误优先于数据错误（对齐 issue.draft_ac
    // 闸序）——缺参与有单两种入参都先撞「agent 未运行」。
    let e = dispatch(&ctx, "board.resume", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(e.contains("agent 未运行"), "actual: {e}");

    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "停车单样例".into(),
            creator: nemesis_board::Actor::system("board"),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();
    let e = dispatch(
        &ctx,
        "board.resume",
        serde_json::json!({ "number": issue.number }),
    )
    .await
    .unwrap_err();
    assert!(e.contains("agent 未运行"), "actual: {e}");

    // L1 注册表：commands() 清单含 board.resume（漂移闸）。
    assert!(BoardHandler.commands().contains(&"board.resume"));

    let _ = std::fs::remove_dir_all(&dir);
}

// 2026-10-16 收件箱重复根修回归钉：dashboard 操作者身份必须是固定
// "dashboard"，不得随 WS session 每连必换——否则每个页面会话以新 admin id
// 落订阅表（唯一键含 subscriber_id，去重失效）并永久累积，通知按订阅者扇出
// 同一事件 N 份（实录：NB-121 一次验收失败收件箱 6 份、全库 18 个历史
// admin 身份）。钉两层：ctx_actor 纯函数契约 + 端到端（同 ctx 不同 session
// 两次评论 → 订阅表仍只有一个 admin 行）。
#[tokio::test]
async fn test_ctx_actor_fixed_dashboard_identity() {
    let dir = unique_dir("ctx-actor-fixed");
    let mut ctx = make_ctx_with_board(&dir);
    ctx.session_id = "rotating-session-abc".into();
    let actor = super::ctx_actor(&ctx);
    assert_eq!(actor.kind, "admin");
    assert_eq!(
        actor.id, "dashboard",
        "admin id 必须固定，不得用 session_id"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_admin_subscriber_dedup_across_sessions() {
    let dir = unique_dir("admin-sub-dedup");
    let ctx = make_ctx_with_board(&dir);
    let store = require_board(&ctx).unwrap();

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "订阅去重样例".into(),
            creator: nemesis_board::Actor::system("board"),
            ..nemesis_board::NewIssue::default()
        })
        .unwrap();

    // 同一 operator 两个不同 WS 会话各评论一次。
    let mut ctx_b = make_ctx_with_board(&dir);
    ctx_b.session_id = "another-session-xyz".into();
    for c in [&ctx, &ctx_b] {
        dispatch(
            c,
            "comment.add",
            serde_json::json!({ "issue_id": issue.id, "content": "op comment" }),
        )
        .await
        .unwrap();
    }

    let subs = store.list_subscribers(issue.id).unwrap();
    let admin_rows: Vec<_> = subs
        .iter()
        .filter(|s| s.subscriber.kind == "admin")
        .collect();
    assert_eq!(
        admin_rows.len(),
        1,
        "两个会话评论后 admin 订阅必须收敛为一行（固定身份），actual: {:?}",
        admin_rows
            .iter()
            .map(|s| &s.subscriber.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(admin_rows[0].subscriber.id, "dashboard");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// A1（2026-10-21 DeepInfer 思路引入）：duration_weighted_candidates 接线
//（匹配器榜 → 耗时次级重排）。核心重排语义（滑窗中位数/回落链/方向）由
// nemesis-board node_perf 单测覆盖；这里钉接线语义：单候选/无样本/跨类别
// 零干预 + 有样本时重排真实生效。
// ---------------------------------------------------------------------------

/// 播种一条完结派发耗时样本（insert 后 hold 秒再 finish，时长 = hold）。
/// `BoardStore.conn` 私有、跨 crate 无法回拨时间戳，快慢差用真实时钟
/// （1s vs 3s，间隔远超调度抖动；秒级粒度下 0 时长会被查询过滤）。
#[cfg(feature = "cluster")]
fn seed_dispatch_duration(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    task_id: &str,
    worker: &str,
    hold_secs: u64,
) {
    store
        .insert_dispatch(
            task_id,
            issue_id,
            worker,
            &nemesis_board::Actor::system("board"),
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(hold_secs));
    assert!(
        store
            .finish_dispatch(task_id, nemesis_board::models::dispatch_state::DONE)
            .unwrap(),
        "finish_dispatch 应命中 dispatched 态"
    );
}

#[cfg(feature = "cluster")]
#[test]
fn a1_duration_weighted_candidates_wiring() {
    let dir = unique_dir("a1-duration-wiring");
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap();

    let prof = store
        .create_issue(nemesis_board::NewIssue {
            title: "职能单".to_string(),
            required_profession: Some("dev:cpp".to_string()),
            ..Default::default()
        })
        .unwrap();
    let plain = store
        .create_issue(nemesis_board::NewIssue {
            title: "无职能单".to_string(),
            ..Default::default()
        })
        .unwrap();

    // 单候选透传（<2 零干预，先于类别读取早退）。
    assert_eq!(
        super::duration_weighted_candidates(&store, &bare_issue(None, &[]), vec!["solo".into()]),
        vec!["solo".to_string()]
    );

    // 空仓零干预：无样本原序返回（零行为变化红线）。
    assert_eq!(
        super::duration_weighted_candidates(
            &store,
            &bare_issue(None, &[]),
            vec!["b".into(), "a".into()]
        ),
        vec!["b".to_string(), "a".to_string()]
    );

    // 真实样本：node-fast 3×1s（中位 1）vs node-slow 3×2s（中位 2），
    // 各达 MIN_WORKER_SAMPLES(=3) 才进 per-worker 预测面（欠采样者落
    // 类别全局中位数，1 样本/worker 会双双打平回原序）。
    for task in ["t-f1", "t-f2", "t-f3"] {
        seed_dispatch_duration(&store, prof.id, task, "node-fast", 1);
    }
    for task in ["t-s1", "t-s2", "t-s3"] {
        seed_dispatch_duration(&store, prof.id, task, "node-slow", 2);
    }

    // 有样本生效：慢者榜首 → 快者提前。
    let ranked = vec!["node-slow".to_string(), "node-fast".to_string()];
    assert_eq!(
        super::duration_weighted_candidates(&store, &prof, ranked.clone()),
        vec!["node-fast".to_string(), "node-slow".to_string()]
    );

    // 跨类别隔离：同批样本（dev:cpp）对无职能单（类别 "_"）零干预。
    assert_eq!(
        super::duration_weighted_candidates(&store, &plain, ranked),
        vec!["node-slow".to_string(), "node-fast".to_string()]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------------------------
// autopilot cron 孤儿清理判据（BUG-20261024-10）：按 `board-ap:{i64}` 全
// 形态判定（autopilot_id_from_job_name），保留命名空间外的撞名用户作业
// 不再被启动同步误删；真孤儿（整数形态 + 未注册）照常清理。
// -------------------------------------------------------------------------

#[test]
fn autopilot_orphan_sweep_spares_user_collision_names_and_sweeps_real_orphans() {
    let dir = unique_dir("ap-sweep-collision");
    let store = Arc::new(BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    let cron_path = dir.join("cron.json").to_string_lossy().to_string();
    let cron = Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(&cron_path),
    ));
    let sched = nemesis_cron::service::CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    };

    // ① 存量撞名作业：修复前用户面可创建 `board-ap:abc`（旧判据按前缀
    //    识别 → 会被启动同步当孤儿删除）。修复后用户面创建被保留命名
    //    空间拒绝，这里用内部旁路模拟存量数据。
    let svc = cron.lock().unwrap();
    let legacy = svc
        .add_job_internal(
            "board-ap:abc",
            sched.clone(),
            "用户消息",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    let plain = svc
        .add_job_internal(
            "plain-user-job",
            sched.clone(),
            "用户消息",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    drop(svc);

    // ② 两条真规则：一条存活，一条随后从 store 删除（造真孤儿）。
    let mk_ap = |name: &str| nemesis_board::NewAutopilot {
        name: name.to_string(),
        cron: "0 9 * * *".to_string(),
        title: "周期任务 {date}".to_string(),
        description: String::new(),
        priority: priority::MEDIUM,
        project_id: None,
        target: String::new(),
        enabled: true,
        auto_plan: false,
        acceptance_criteria: None,
    };
    let ap_live = store.create_autopilot(&mk_ap("存活规则")).unwrap();
    let ap_orphan = store.create_autopilot(&mk_ap("将删除规则")).unwrap();
    sync_autopilot_jobs(&cron, &store).unwrap();
    let (live_job_id, orphan_job_id) = {
        let live = store.get_autopilot(ap_live.id).unwrap();
        let orphan = store.get_autopilot(ap_orphan.id).unwrap();
        (
            live.cron_job_id.clone().expect("存活规则应已装配"),
            orphan.cron_job_id.clone().expect("待删规则应已装配"),
        )
    };
    assert_ne!(live_job_id, orphan_job_id);

    store.remove_autopilot(ap_orphan.id).unwrap();
    sync_autopilot_jobs(&cron, &store).unwrap();

    // ③ 断言：真孤儿作业被清理；存量撞名/普通用户作业全部幸存；存活
    //    规则的作业保留。
    let after: Vec<(String, String)> = {
        let svc = cron.lock().unwrap();
        svc.list_jobs(true)
            .into_iter()
            .map(|j| (j.id, j.name))
            .collect()
    };
    assert!(
        !after.iter().any(|(id, _)| id == &orphan_job_id),
        "真孤儿作业应被清理: {after:?}"
    );
    for (id, label) in [
        (&legacy.id, "存量撞名作业 board-ap:abc"),
        (&plain.id, "普通用户作业"),
        (&live_job_id, "存活规则的作业"),
    ] {
        assert!(
            after.iter().any(|(jid, _)| jid == id),
            "{label} 不应被孤儿清理误删: {after:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn autopilot_id_from_job_name_shape_unit() {
    assert_eq!(autopilot_id_from_job_name("board-ap:42"), Some(42));
    assert_eq!(autopilot_id_from_job_name("board-ap:0"), Some(0));
    // 非整数形态 / 负数 / 前缀异形 / 无后缀 → 非 autopilot 作业。
    assert_eq!(autopilot_id_from_job_name("board-ap:abc"), None);
    // 负数也是整数形态（i64 parse 接受）——解析出 Some(-1) 无害：规则 id
    // 是自增正整数，get_autopilot(-1) 必不命中，消费侧自然失败。
    assert_eq!(autopilot_id_from_job_name("board-ap:-1"), Some(-1));
    assert_eq!(autopilot_id_from_job_name("my-board-ap:5"), None);
    assert_eq!(autopilot_id_from_job_name("board-ap:"), None);
    assert_eq!(autopilot_id_from_job_name("memory-dreaming:sweep"), None);
}

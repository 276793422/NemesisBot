//! Board handler dispatch 级测试（裸命令名经 `handle_cmd`，统一闸门硬性项）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_board::models::priority;
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
    // 创建者（admin/test-session）+ 手动订阅（同一 admin 身份 → 幂等一条）。
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
        acceptance_criteria: format!("- [ ] {title} 验收"),
        depends_on: deps,
    }
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn issue_plan_missing_id_rejected() {
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
    assert_eq!(channels.len(), 3, "default #dev/#qa/#general seeded");
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
        assert_eq!(calls[0].0, "admin/test-session");
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
        .create_project("联动项目", "", None, "", "")
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
    // F2 回归（T32 实证缺口）：confirm_plan 落库的子单必须继承父单
    // project_id——派发联动 link_project_on_dispatch 读的是子单自己的
    // project_id，缺继承则项目永远停在 active。
    let dir = unique_dir("confirm-plan-inherit-project");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();
    let pid = store
        .create_project("继承项目", "", None, "", "")
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
            acceptance_criteria: String::new(),
            depends_on: Vec::new(),
        }],
        &nemesis_board::Actor::system("board"),
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
    let pid = store.create_project(name, "", None, "", "").unwrap().id;
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
    let empty = store.create_project("空项目", "", None, "", "").unwrap().id;
    assert!(!project_completion_eligible(&store, empty));

    // 全部顶层父单 done → true（F2 首派联动后项目已在 in_progress 主链）。
    let all_done = f3_project_with_parents(&store, "全done项目", 2, 2);
    assert!(project_completion_eligible(&store, all_done));

    // 存在未 done 顶层父单 → false。
    let partial = f3_project_with_parents(&store, "半done项目", 2, 1);
    assert!(!project_completion_eligible(&store, partial));

    // 子单 done 但父单未 done → false（只有顶层父单是触发面）。
    let pid = store
        .create_project("子done项目", "", None, "", "")
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
    use std::sync::Mutex;

    let dir = unique_dir("f3-notify");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let fired: &'static Mutex<Vec<i64>> = Box::leak(Box::new(Mutex::new(Vec::new())));
    // 进程级 OnceLock：单测二进制内只有本测试注册（守卫用例在 hook get 前
    // 就早退）；set 失败 = 本用例先前轮次已注册同一 recording 闭包，直接用。
    let _ = set_project_review_hook(std::sync::Arc::new(|pid| {
        fired.lock().unwrap().push(pid);
    }));

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
    assert_eq!(out["review"]["max_turns"], 1);
    assert_eq!(out["review"]["selfcheck"], false);
    assert_eq!(out["review"]["auto_close_project"], false);
    assert_eq!(out["budget"]["max_subissues_per_parent"], 20);
    assert_eq!(out["budget"]["max_total_redispatch"], 0);
    assert_eq!(out["budget"]["wall_clock_budget_secs"], 0);
    assert_eq!(out["review"]["checkers"], 1);
    assert_eq!(out["budget"]["max_tokens_per_parent"], 0);

    // 八键 set → 盘上真实落盘回读。
    for (key, value) in [
        ("review.max_turns", serde_json::json!(3)),
        ("review.selfcheck", serde_json::json!(true)),
        ("review.auto_close_project", serde_json::json!(true)),
        ("budget.max_subissues_per_parent", serde_json::json!(10)),
        ("budget.max_total_redispatch", serde_json::json!(5)),
        ("budget.wall_clock_budget_secs", serde_json::json!(3600)),
        ("review.checkers", serde_json::json!(3)),
        ("budget.max_tokens_per_parent", serde_json::json!(100000)),
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
        },
        dir.join("ws"),
    ))
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
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["python".into()],
        node_type: "agent".into(),
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

/// 构造兜底开关配置（显式入参，测试不碰进程级全局 live config）。
#[cfg(feature = "cluster")]
fn fallback_cfg(on: bool, target: Option<&str>) -> nemesis_config::BoardFlagConfig {
    nemesis_config::BoardFlagConfig {
        dispatch_fallback: on,
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
    let dir = unique_dir("fallback-off");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb0");
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(false, None)),
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
    let dir = unique_dir("fallback-no-peer");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb1");
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(true, None)),
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

/// 开关开 + 在线节点不带 python：松弛兜底派出 + ⚠ 评论留痕；父单不转
/// blocked（首派即走，blocked 联动只属于停车路径）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn fallback_dispatches_to_relaxed_online_peer() {
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
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec!["rust".into()],
        node_type: "agent".into(),
    });

    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(true, None)),
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
        role: NodeRole::Worker,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        node_type: "agent".into(),
    });

    // 钉住 name（大小写不敏感）→ 派给该节点。
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);
    let out = super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(true, Some("alex"))),
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
        Some(&fallback_cfg(true, Some("Ghost"))),
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
        role: NodeRole::Coordinator,
        category: "development".into(),
        capabilities: vec![],
        tags: vec![],
        node_type: "agent".into(),
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
        Some(&fallback_cfg(true, None)),
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
    let dir = unique_dir("fallback-sweep");
    let ctx = make_ctx_with_board(&dir);
    let store = store_of(&ctx);
    let actor = Actor::admin("test-session");
    let cluster = offline_cluster(&dir, "coord-fb5");

    // 开关关时先停车。
    let parent = no_parent(&store);
    let a = planner_child(&store, parent, "子A", vec!["python".to_string()]);
    super::dispatch_subissue_auto_with_config(
        Some(&fallback_cfg(false, None)),
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
        role: NodeRole::Worker,
        category: "general".into(),
        capabilities: vec![],
        tags: vec![],
        node_type: "agent".into(),
    });
    let (cands, dispatched, failed) = super::sweep_parked_dispatches_with_config(
        Some(&fallback_cfg(true, None)),
        &store,
        &cluster,
        &actor,
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

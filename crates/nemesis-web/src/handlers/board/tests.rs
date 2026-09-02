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
    let service = nemesis_board::BoardService::new(Arc::new(store), role);
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
    assert!(dispatch(&ctx, "issue.get", serde_json::json!({})).await.is_err());

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
    let out = dispatch(&ctx, "issue.status", serde_json::json!({ "id": id, "status": "in_progress" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["issue"]["status"], "in_progress");
    // 转移写入 status_change 评论。
    assert!(out["issue"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["ctype"] == "status_change"));

    // 非法：in_progress → backlog
    let err = dispatch(&ctx, "issue.status", serde_json::json!({ "id": id, "status": "backlog" }))
        .await
        .expect_err("illegal transition must be rejected");
    assert!(err.contains("非法状态转移"), "{err}");

    // 未知 status 字符串
    let err = dispatch(&ctx, "issue.status", serde_json::json!({ "id": id, "status": "bogus" }))
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
    assert!(dispatch(&ctx, "issue.update", serde_json::json!({ "title": "x" }))
        .await
        .is_err());

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

    dispatch(&ctx, "comment.add", serde_json::json!({ "issue_id": id, "content": "第一条" }))
        .await
        .expect("comment.add should succeed");
    // 空评论被拒（store 校验透传）。
    assert!(dispatch(&ctx, "comment.add", serde_json::json!({ "issue_id": id, "content": "  " }))
        .await
        .is_err());

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

    dispatch(&ctx, "subscriber.add", serde_json::json!({ "issue_id": id }))
        .await
        .expect("subscriber.add should succeed");
    let out = dispatch(&ctx, "subscriber.list", serde_json::json!({ "issue_id": id }))
        .await
        .unwrap()
        .unwrap();
    let subs = out["subscribers"].as_array().unwrap();
    // 创建者（admin/test-session）+ 手动订阅（同一 admin 身份 → 幂等一条）。
    assert!(subs.iter().any(|s| s["subscriber"]["kind"] == "admin"));
    dispatch(&ctx, "subscriber.remove", serde_json::json!({ "issue_id": id }))
        .await
        .expect("subscriber.remove should succeed");
    let out = dispatch(&ctx, "subscriber.list", serde_json::json!({ "issue_id": id }))
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
    assert!(dispatch(&ctx, "project.create", serde_json::json!({ "name": "主项目" }))
        .await
        .is_err());
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
    let out = dispatch(&ctx, "attachment.list", serde_json::json!({ "issue_id": id }))
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
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "worker 写入", "priority": 2 }))
        .await
        .expect("worker must create issues locally");
    let issue = &out.unwrap()["issue"];
    assert_eq!(issue["number"], "NB-1");
    let id = issue["id"].as_i64().unwrap();

    let out = dispatch(&ctx, "issue.update", serde_json::json!({ "id": id, "priority": 3 }))
        .await
        .expect("worker must update issues locally");
    assert_eq!(out.unwrap()["issue"]["priority"], 3);

    dispatch(&ctx, "comment.add", serde_json::json!({ "issue_id": id, "content": "来自 worker" }))
        .await
        .expect("worker must add comments locally");

    let out = dispatch(&ctx, "issue.status", serde_json::json!({ "id": id, "status": "todo" }))
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
        ("issue.status", serde_json::json!({ "id": 1, "status": "todo" })),
        ("issue.move", serde_json::json!({ "id": 1, "status": "todo", "position": 0 })),
        ("issue.dispatch", serde_json::json!({ "id": 1 })),
        ("issue.cancel", serde_json::json!({ "id": 1 })),
        ("comment.add", serde_json::json!({ "issue_id": 1, "content": "x" })),
        ("subscriber.add", serde_json::json!({ "issue_id": 1 })),
        ("subscriber.remove", serde_json::json!({ "issue_id": 1 })),
        ("project.create", serde_json::json!({ "name": "p" })),
        ("project.update", serde_json::json!({ "id": 1 })),
        ("attachment.add", serde_json::json!({ "issue_id": 1, "filename": "a.txt", "content": "eA==" })),
        ("inbox.mark_read", serde_json::json!({ "all": true })),
        ("autopilot.create", serde_json::json!({ "name": "ap", "title": "t", "cron": "0 9 * * *" })),
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
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "权威节点" }))
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
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "派发目标校验" }))
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
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "终态派发" }))
        .await
        .unwrap()
        .unwrap();
    let id2 = out["issue"]["id"].as_i64().unwrap();
    dispatch(&ctx, "issue.status", serde_json::json!({ "id": id2, "status": "done" }))
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
    assert!(out["issue"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["ctype"] != "status_change"));
    let out = dispatch(&ctx, "activity.list", serde_json::json!({ "issue_id": id }))
        .await
        .unwrap()
        .unwrap();
    assert!(out["activity"].as_array().unwrap().iter().any(|a| a["action"] == "reordered"));

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
    assert!(out["issue"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["ctype"] == "status_change"));

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
    let err = dispatch(&ctx, "issue.move", serde_json::json!({ "id": id, "status": "todo" }))
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
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "带附件" }))
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
    assert!(storage_path.starts_with("board/files/issue_"), "{storage_path}");
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
    let out = dispatch(&ctx, "attachment.list", serde_json::json!({ "issue_id": id }))
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
    assert!(dispatch(
        &ctx,
        "attachment.add",
        serde_json::json!({ "issue_id": 99999, "filename": "x.txt", "content": "aGVsbG8=" }),
    )
    .await
    .is_err());
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
    let err = dispatch(&ctx, "project.update", serde_json::json!({ "id": pid, "name": "  " }))
        .await
        .expect_err("empty name must be rejected");
    assert!(err.contains("must not be empty"), "{err}");

    // 缺 id。
    assert!(dispatch(&ctx, "project.update", serde_json::json!({ "name": "x" }))
        .await
        .is_err());

    // 不存在的项目。
    let err = dispatch(&ctx, "project.update", serde_json::json!({ "id": 99999, "name": "x" }))
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
    let out = dispatch(&ctx, "inbox.list", serde_json::json!({ "unread_only": true, "limit": 1 }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["notifications"].as_array().unwrap().len(), 1);

    // 单条已读（幂等：第二次 marked=0）。
    let first_id = list[0]["id"].as_i64().unwrap();
    let out = dispatch(&ctx, "inbox.mark_read", serde_json::json!({ "id": first_id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["marked"], 1);
    assert_eq!(out["unread"], 1);
    let out = dispatch(&ctx, "inbox.mark_read", serde_json::json!({ "id": first_id }))
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
    let out = dispatch(&ctx, "inbox.list", serde_json::json!({ "unread_only": true }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["notifications"].as_array().unwrap().len(), 0);

    // 缺 id 且无 all → 报错。
    assert!(dispatch(&ctx, "inbox.mark_read", serde_json::json!({}))
        .await
        .is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 P4 issue.cancel 校验矩阵（无集群实例）：无派发先拒、集群缺失在动账
/// 前拒（取消 = 下行 task_cancel，A 侧竞态守卫在集群校验之后才动账）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_issue_cancel_validation_without_cluster() {
    let dir = unique_dir("cancel-validation");
    let ctx = make_ctx_with_board(&dir);
    let store = ctx.state.board.as_ref().unwrap().store().clone();

    // 建 issue → 无派发 → 取消报「没有进行中的派发」。
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "待取消" }))
        .await
        .unwrap()
        .unwrap();
    let id = out["issue"]["id"].as_i64().unwrap();
    let err = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": id }))
        .await
        .expect_err("no active dispatch must error");
    assert!(err.contains("没有进行中的派发"), "{err}");

    // 直播一条 dispatched 派发（绕过集群）→ 集群缺失报错。
    let actor = nemesis_board::Actor::admin("test-session");
    store
        .insert_dispatch("task-cancel-1", id, "node-b", &actor)
        .expect("seed dispatch");
    let err = dispatch(&ctx, "issue.cancel", serde_json::json!({ "id": id }))
        .await
        .expect_err("missing cluster must error");
    assert!(err.contains("集群未运行"), "{err}");

    // 未动账：派发仍在、issue 状态未变。
    assert!(store.get_active_dispatch(id).unwrap().is_some());
    let out = dispatch(&ctx, "issue.get", serde_json::json!({ "id": id }))
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
    let out = dispatch(&ctx, "autopilot.update", serde_json::json!({ "id": ap_id, "enabled": false }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(out["autopilot"]["enabled"], false);
    assert_eq!(out["autopilot"]["title"], "日报 {date}");
    assert!(dispatch(
        &ctx,
        "autopilot.update",
        serde_json::json!({ "id": ap_id, "cron": "bad" })
    )
    .await
    .is_err());

    // 手动 run（target 空、enabled=false 也可手动触发）：仅建单 + {date} 替换。
    let out = dispatch(&ctx, "autopilot.run", serde_json::json!({ "id": ap_id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["ran"], true);
    assert!(out["dispatch"].is_null());
    let number = out["issue_number"].as_str().unwrap().to_string();
    assert!(!number.contains("{date}"), "placeholder must be substituted: {number}");

    // target 非空 + 集群缺失 → 建单前拒绝（不留半成品）。拒绝臂随编译形态：
    // cluster 编译但未运行（nemesisbot 全量常态）→「集群未运行」；cluster
    // 未编译（minimal-iot 裁剪档；nightly --exclude nemesisbot 后 feature
    // 统一不再透传，同形态）→「cluster feature 未编译」。两者都是正确生产
    // 行为，按实际编译形态断言对应臂（2026-09-02 CI 实录：本测试未像
    // dispatch/cancel 测试那样整体门控，在无 cluster 编译下钉死单臂假红）。
    let err = dispatch(&ctx, "autopilot.run", serde_json::json!({ "id": ap_dispatch_id }))
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
    assert!(dispatch(&ctx, "autopilot.run", serde_json::json!({ "id": ap_id }))
        .await
        .is_err());

    // 缺 id。
    assert!(dispatch(&ctx, "autopilot.update", serde_json::json!({ "enabled": true }))
        .await
        .is_err());

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
    assert!(!should_auto_dispatch(Some(&off), Some(AssignmentType::Worker)));
    // 开关开 + worker 指派 → 触发。
    let on = nemesis_config::BoardFlagConfig {
        auto_dispatch: true,
        ..Default::default()
    };
    assert!(should_auto_dispatch(Some(&on), Some(AssignmentType::Worker)));
    // 开关开但非 worker（manager_self / 未指派）→ 不触发。
    assert!(!should_auto_dispatch(Some(&on), Some(AssignmentType::ManagerSelf)));
    assert!(!should_auto_dispatch(Some(&on), None));
}

#[tokio::test]
async fn assign_worker_default_stays_pending_no_dispatch() {
    // 默认（无全局 config store → live_board_config None）：指派给 worker
    // 只写 assignee 元数据，不触发派发——行为与 W2.5 之前完全一致
    //（不要求集群、状态不推进、无 ⛔ 评论）。
    let dir = unique_dir("assign-default-off");
    let ctx = make_ctx_with_board(&dir);
    let out = dispatch(&ctx, "issue.create", serde_json::json!({ "title": "默认关" }))
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
    let dispatched = super::auto_dispatch_with_config(
        Some(&on),
        &store,
        None,
        &issue,
        &actor,
    );
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

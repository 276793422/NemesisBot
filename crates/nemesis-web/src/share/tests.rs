//! share.rs 测试 —— 分享存储 roundtrip / 撤销 / 白名单投影 / HTTP 三态。
//!
//! 白名单投影是安全命门：images 只透数量、file_changes/checkpoint_turn/
//! cron_* 全丢弃、非 user/assistant 行跳过 —— 每条都有断言钉住。
//!
//! 隔离（家规，同 fork_route_tests.rs）：chat_log 走全局 default path
//! manager，测试用 nanos 唯一 session key + delete_chat_log 清理；分享
//! 存储走独立 tempdir workspace，不碰全局态。

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// env 测试竞争锁惯例：进程全局一把锁（本模块的存储测试写各自 tempdir，
// 但 HTTP 测试经全局 chat_log，串行化更稳）。
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn temp_workspace() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn nanos_sid() -> String {
    format!(
        "share{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn make_state(ws: &str) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.to_string()),
        home: Some(ws.to_string()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
        // AppState 双声明字段（workflow-gated 真 / Arc<()> stub）——按
        // feature 组合构造，保证 --no-default-features 也编译。
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    })
}

#[test]
fn test_create_and_resolve_roundtrip() {
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let entry = create_share(ws.path().to_str().unwrap(), "abc123").unwrap();
    assert_eq!(entry.session_id, "abc123");
    assert!(!entry.revoked);
    // token = 32 hex（uuid simple）
    assert_eq!(entry.token.len(), 32);
    assert!(entry.token.chars().all(|c| c.is_ascii_hexdigit()));

    let resolved = resolve_share(ws.path().to_str().unwrap(), &entry.token).unwrap();
    assert_eq!(resolved, entry);
}

#[test]
fn test_create_is_idempotent_per_session() {
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let p = ws.path().to_str().unwrap();
    let a = create_share(p, "s1").unwrap();
    let b = create_share(p, "s1").unwrap();
    assert_eq!(a, b, "同会话重复创建应复用现有 token");
    assert_eq!(list_shares(p).len(), 1);
}

#[test]
fn test_revoked_share_not_resolvable_and_not_reused() {
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let p = ws.path().to_str().unwrap();
    let e = create_share(p, "s1").unwrap();
    assert!(revoke_share(p, &e.token).unwrap());
    assert!(resolve_share(p, &e.token).is_none(), "撤销后必须解析不到");
    // 重新创建 → 新 token（撤销即作废，不复用）
    let e2 = create_share(p, "s1").unwrap();
    assert_ne!(e.token, e2.token);
    assert_eq!(list_shares(p).len(), 2);
    // 未知 token 撤销 → false
    assert!(!revoke_share(p, "nonexistent").unwrap());
}

#[test]
fn test_create_sanitizes_session_id_no_path_traversal() {
    // 安全回归（2026-09-07）：sid 拼进 read_chat_log 的文件路径
    // （log_path 只替换 `:`），含分隔符的裸 sid 可路径穿越读任意 jsonl。
    // create_share 必须就地消毒（与 sessions export/rewind 臂同源）。
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let p = ws.path().to_str().unwrap();

    // 显式期望（sanitize 白名单 = [alnum - _]，其余一律 `_`）：
    // `../../evil` = 6 个非白名单字符 + evil → `______evil`。
    let e = create_share(p, "../../evil").unwrap();
    assert_eq!(e.session_id, "______evil");
    // `a/b/c` → `a_b_c`；同形输入幂等复用同一条记录。
    let a = create_share(p, "a/b/c").unwrap();
    assert_eq!(a.session_id, "a_b_c");
    let b = create_share(p, "a_b_c").unwrap();
    assert_eq!(a, b, "消毒应让恶意输入与手工合法输入收敛到同一条");
}

#[test]
fn test_projection_whitelist_only() {
    let rows = vec![
        serde_json::json!({
            "role": "user", "content": "帮我看看", "timestamp": "2026-09-07T10:00:00Z",
            "images": [{"path": "C:/secret/local/pic.png"}]
        }),
        serde_json::json!({
            "role": "assistant", "content": "好的", "timestamp": "2026-09-07T10:00:05Z",
            "model": "test/testai-1.1",
            "file_changes": [{"path": "C:/workspace/src/main.rs", "kind": "Edit"}],
            "checkpoint_turn": 3, "cron_job_id": "j1", "cron_job_name": "daily"
        }),
        serde_json::json!({"role": "tool", "content": "tool output", "timestamp": "t"}),
        serde_json::json!({"role": "error", "content": "boom", "timestamp": "t"}),
    ];
    let out = project_messages(&rows);
    assert_eq!(out.len(), 2, "tool/error 行必须跳过");
    let user = &out[0];
    assert_eq!(user["role"], "user");
    assert_eq!(user["content"], "帮我看看");
    assert_eq!(user["image_count"], 1, "images 只透数量");
    let s = user.to_string();
    assert!(
        !s.contains("pic.png"),
        "图片本地路径绝不能出现在投影里: {s}"
    );
    assert!(!s.contains("C:"), "绝对路径泄漏");

    let asst = &out[1];
    let s = asst.to_string();
    assert_eq!(asst["model"], "test/testai-1.1");
    assert!(!s.contains("main.rs"), "file_changes 路径泄漏");
    assert!(!s.contains("file_changes"));
    assert!(!s.contains("checkpoint_turn"));
    assert!(!s.contains("cron_job"));
}

#[test]
fn test_projection_empty_and_no_model() {
    let rows = vec![
        serde_json::json!({"role": "assistant", "content": "无 model 字段", "timestamp": "t"}),
        serde_json::json!({"role": "user", "content": "空 images", "timestamp": "t", "images": []}),
    ];
    let out = project_messages(&rows);
    assert_eq!(out.len(), 2);
    assert!(out[0].get("model").is_none());
    assert!(out[0].get("image_count").is_none());
    assert!(
        out[1].get("image_count").is_none(),
        "空 images 不该写 image_count"
    );
    // 投影是纯函数：rows 为空 → 空
    assert!(project_messages(&[]).is_empty());
}

// --- HTTP handler 三态（unknown / 会话已删 / live 视图 + 撤销后 404）---

async fn call_share(
    state: Arc<AppState>,
    token: &str,
) -> Result<serde_json::Value, (axum::http::StatusCode, serde_json::Value)> {
    let res = handle_api_share(
        axum::extract::Path(token.to_string()),
        axum::extract::State(state),
    )
    .await;
    match res {
        Ok(axum::Json(v)) => Ok(v),
        Err((status, axum::Json(v))) => Err((status, v)),
    }
}

#[tokio::test]
async fn test_http_share_unknown_token_404() {
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let state = make_state(ws.path().to_str().unwrap());
    let (status, body) = call_share(state, "deadbeef").await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("分享"));
}

#[tokio::test]
async fn test_http_share_session_deleted_404() {
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let p = ws.path().to_str().unwrap();
    let state = make_state(p);
    let e = create_share(p, "ghost").unwrap();
    // 无 jsonl → read_chat_log 空 → 404
    let (status, body) = call_share(state, &e.token).await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("不存在") || msg.contains("为空"), "got: {msg}");
}

#[tokio::test]
async fn test_http_share_live_view_whitelisted_then_revoke_404() {
    let _g = TEST_LOCK.lock();
    let ws = temp_workspace();
    let p = ws.path().to_str().unwrap();
    let sid = nanos_sid();
    let key = format!("agent:main:session:{sid}");
    nemesis_agent::chat_log::append_chat_log(&key, "user", "你好");
    nemesis_agent::chat_log::append_chat_log_with_model(
        &key,
        "assistant",
        "你好！有什么可以帮你？",
        Some("test/testai-1.1"),
    );

    let state = make_state(p);
    let e = create_share(p, &sid).unwrap();
    let body = call_share(state.clone(), &e.token).await.unwrap();
    assert_eq!(body["view"], "live");
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2, "tool/system 行不透出");
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[1]["content"], "你好！有什么可以帮你？");
    assert_eq!(msgs[1]["model"], "test/testai-1.1");

    // 撤销后同一 token → 404
    revoke_share(p, &e.token).unwrap();
    let (status, _) = call_share(state, &e.token).await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    nemesis_agent::chat_log::delete_chat_log(&key);
}

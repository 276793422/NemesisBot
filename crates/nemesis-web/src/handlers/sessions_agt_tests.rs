//! sessions.rs AGT 覆盖率批次（2026-09-25）。与 extra/s10b/share 三套互补，
//! 聚焦仍缺的确定性臂：
//! - `mark_delivered` 的缺参/错型臂 + 正常清零臂
//! - `create` 的 binding_key 原子 get-or-create 全链（显式标题、缺省标题、
//!   纯空白键回落 plain create）+ plain create 的标题提取/缺省臂
//! - `set_binding` / `remove_binding` 的缺参/错型臂 + 全链
//! - `rename` 带 title 的 happy path（test_home::lock_home() 重定向单例
//!   home——遵循 s10b 的 clear/delete 同款纪律）
//! - `rewind_to_message` / `redo` 的缺参 bail 臂
//!
//! 归属写盘注意：create/rename 的 meta sidecar 与 chat_log 单例都经
//! test_home::lock_home() 重定向；workspace 侧 artifacts（jsonl/meta）
//! 直接写在调用方 tempdir 的 `<ws>/logs/session_logs/` 下。

use super::sessions::SessionsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn unique_sid(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    SessionsHandler.handle_cmd(cmd, data, ctx).await
}

/// 在 workspace 侧 session_logs 落一个存活标记 + meta 标题
/// （session_artifact_exists / session_title 的读侧都在 workspace）。
fn seed_ws_artifact(ws: &std::path::Path, sid: &str, title: &str) {
    let logs = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join(format!("agent_main_session_{sid}.jsonl")),
        "{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":\"2026-09-25T00:00:00+08:00\"}\n",
    )
    .unwrap();
    std::fs::write(
        logs.join(format!("agent_main_session_{sid}.meta.json")),
        serde_json::json!({ "title": title }).to_string(),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// mark_delivered：缺参 / 错型 / 正常清零
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_mark_delivered_guards_and_clear() {
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // data 缺 / 键缺 / 键非字符串 → 同一 "missing session_id" 诚实报错。
    for data in [
        None,
        Some(serde_json::json!({})),
        Some(serde_json::json!({"session_id": 123})),
    ] {
        let err = run(&ctx, "mark_delivered", data).await.unwrap_err();
        assert_eq!(err, "missing session_id");
    }

    // 正常臂：无未送达记录 → cleared false（幂等，布尔而非计数）。
    let sid = unique_sid("agtdl");
    let out = run(
        &ctx,
        "mark_delivered",
        Some(serde_json::json!({ "session_id": sid })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["cleared"], false, "{out}");
    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&sid)
    );
    nemesis_agent::chat_log::delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// create：binding_key 原子 get-or-create 全链 + plain 标题臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_create_binding_key_roundtrip_and_plain_title_arms() {
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let ctx = make_ctx(&dir);

    // ① binding_key + 显式标题：新建（manual meta 即写）。
    let out = run(
        &ctx,
        "create",
        Some(serde_json::json!({ "binding_key": "agt-bk-1", "title": " AGT Named " })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["reused"], false, "{out}");
    assert_eq!(out["title"], "AGT Named", "trim 生效");
    let sid1 = out["session_id"].as_str().unwrap().to_string();

    // workspace 侧补存活标记 + live 标题 → 复用臂读得到。
    seed_ws_artifact(&ws, &sid1, "AGT Live");

    // ② 同键再次 create：幂等命中既有会话（零新建），标题实时解析。
    let out = run(
        &ctx,
        "create",
        Some(serde_json::json!({ "binding_key": "agt-bk-1", "title": "ignored" })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["reused"], true, "{out}");
    assert_eq!(out["session_id"], serde_json::json!(sid1));
    assert_eq!(out["title"], "AGT Live");

    // ③ binding_key + 缺标题：占位符标题（unwrap_or_else 闭包臂）。
    let out = run(
        &ctx,
        "create",
        Some(serde_json::json!({ "binding_key": "agt-bk-2" })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["reused"], false);
    assert_eq!(out["title"], nemesis_agent::chat_log::DEFAULT_SESSION_TITLE);
    let sid3 = out["session_id"].as_str().unwrap().to_string();
    let key3 = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&sid3)
    );
    nemesis_agent::chat_log::delete_chat_log(&key3);

    // ④ 纯空白 binding_key：filter 落空 → 回落 plain create（uuid 新会话）。
    let out = run(
        &ctx,
        "create",
        Some(serde_json::json!({ "binding_key": "   " })),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(out.get("reused").is_none(), "plain 形态无 reused: {out}");
    assert_eq!(out["title"], nemesis_agent::chat_log::DEFAULT_SESSION_TITLE);
    let sid4 = out["session_id"].as_str().unwrap().to_string();
    let key4 = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&sid4)
    );
    nemesis_agent::chat_log::delete_chat_log(&key4);

    // ⑤ plain create + 显式标题：提取链 + manual meta（E7 用户意志）。
    let out = run(
        &ctx,
        "create",
        Some(serde_json::json!({ "title": " Plain Named " })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["title"], "Plain Named");
    let sid5 = out["session_id"].as_str().unwrap().to_string();
    let key5 = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&sid5)
    );
    nemesis_agent::chat_log::delete_chat_log(&key5);
}

// ---------------------------------------------------------------------------
// set_binding / remove_binding：缺参 / 错型 / 全链
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_set_and_remove_binding_guards_and_roundtrip() {
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let ctx = make_ctx(&dir);

    // set_binding 守卫：键缺 / 键非串 / sid 缺 / sid 非串。
    for data in [
        Some(serde_json::json!({})),
        Some(serde_json::json!({ "binding_key": 5 })),
        Some(serde_json::json!({ "binding_key": "k" })),
        Some(serde_json::json!({ "binding_key": "k", "session_id": 5 })),
    ] {
        let err = run(&ctx, "set_binding", data).await.unwrap_err();
        assert!(
            err == "missing binding_key" || err == "missing session_id",
            "got: {err}"
        );
    }

    // set_binding 全链：目标会话必须存活（workspace 侧 artifact）。
    let sid = unique_sid("agtbind");
    seed_ws_artifact(&ws, &sid, "bound");
    let out = run(
        &ctx,
        "set_binding",
        Some(serde_json::json!({ "binding_key": " agt-bk-r ", "session_id": sid })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(out["binding_key"], "agt-bk-r", "trim 生效");
    assert_eq!(out["session_id"], serde_json::json!(sid));

    // remove_binding 守卫：键缺 / 非串。
    for data in [
        Some(serde_json::json!({})),
        Some(serde_json::json!({ "binding_key": 9 })),
    ] {
        let err = run(&ctx, "remove_binding", data).await.unwrap_err();
        assert_eq!(err, "missing binding_key");
    }

    // 摘键：先 true（存在）再 false（幂等缺席）。
    let out = run(
        &ctx,
        "remove_binding",
        Some(serde_json::json!({ "binding_key": "agt-bk-r" })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["removed"], true);
    let out = run(
        &ctx,
        "remove_binding",
        Some(serde_json::json!({ "binding_key": "agt-bk-r" })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["removed"], false);
}

// ---------------------------------------------------------------------------
// rename 带 title 的 happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_rename_with_title_writes_manual_meta() {
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // title 非字符串 → 同一 "missing title"。
    let err = run(
        &ctx,
        "rename",
        Some(serde_json::json!({ "session_id": "s1", "title": 5 })),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "missing title");

    // happy path：session_id 原样回显 + 标题原样透传（handler 不 trim，
    // trim 只发生在 create/rename 的标题提取闭包之外的 meta 写侧语义）。
    let sid = unique_sid("agtrn");
    let out = run(
        &ctx,
        "rename",
        Some(serde_json::json!({ "session_id": sid, "title": " Renamed " })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["session_id"], serde_json::json!(sid));
    assert_eq!(out["title"], " Renamed ");
    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&sid)
    );
    nemesis_agent::chat_log::delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// rewind_to_message / redo：缺参 bail（参数解析在前，loop 检查在后）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_rewind_to_message_and_redo_param_guards() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // rewind_to_message：sid 缺 → index 缺。
    let err = run(&ctx, "rewind_to_message", Some(serde_json::json!({})))
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");
    let err = run(
        &ctx,
        "rewind_to_message",
        Some(serde_json::json!({ "session_id": "s1" })),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "missing message_index");
    // index 非数值 → 同一 bail。
    let err = run(
        &ctx,
        "rewind_to_message",
        Some(serde_json::json!({ "session_id": "s1", "message_index": "x" })),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "missing message_index");

    // redo：sid 缺 / 非串。
    let err = run(&ctx, "redo", Some(serde_json::json!({})))
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");
    let err = run(&ctx, "redo", Some(serde_json::json!({ "session_id": 7 })))
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");
}

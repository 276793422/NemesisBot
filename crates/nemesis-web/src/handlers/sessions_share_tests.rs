//! L4 会话分享（2026-09-07）—— sessions 模块 share_create/share_list/
//! share_revoke 三命令的 WSAPI handler 测试。
//!
//! 存储走 tempdir workspace（crate::share 单元测试已覆盖存储语义），这里
//! 钉 handler 层：参数缺失 bail、create→list→revoke 全链路、未知 token
//! revoke 的诚实报错。

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
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn share_commands_registered_and_bail_on_missing_args() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SessionsHandler;

    // L1 纪律：注册表必须列全三命令。
    let cmds = h.commands();
    for c in ["share_create", "share_list", "share_revoke"] {
        assert!(cmds.contains(&c), "commands() 缺 {c}");
    }

    // share_create 缺 session_id。
    let err = h.handle_cmd("share_create", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing session_id");

    // share_revoke 缺 token。
    let err = h.handle_cmd("share_revoke", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing token");
}

#[tokio::test]
async fn share_create_list_revoke_full_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SessionsHandler;

    // create → token + path。
    let out = h
        .handle_cmd(
            "share_create",
            Some(serde_json::json!({ "session_id": "sid1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let token = out["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 32);
    assert_eq!(out["path"], format!("/share/?t={token}"));

    // 幂等：同会话再 create → 同 token。
    let out2 = h
        .handle_cmd(
            "share_create",
            Some(serde_json::json!({ "session_id": "sid1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out2["token"], out["token"]);

    // list → 一条记录（会话不存在 → title null，live 语义诚实缺省）。
    let out = h
        .handle_cmd("share_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let shares = out["shares"].as_array().unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["token"], token.as_str());
    assert_eq!(shares[0]["revoked"], false);
    assert_eq!(shares[0]["session_id"], "sid1");

    // revoke → ok:true；再 list → revoked:true；再 revoke → 诚实报错。
    let out = h
        .handle_cmd(
            "share_revoke",
            Some(serde_json::json!({ "token": token })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["ok"], true);

    let out = h
        .handle_cmd("share_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["shares"][0]["revoked"], true);

    let err = h
        .handle_cmd(
            "share_revoke",
            Some(serde_json::json!({ "token": "nope" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("不存在"));
}

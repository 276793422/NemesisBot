//! selfupdate WSAPI handler 测试（轻面）：命令清单一致性、未知命令、status
//! 形状（身份槽未装配 = 诚实空值）。apply 的受理/未装配分支随 feature 与
//! manager 槽——纯槽态断言离线可达：未装配时 apply 必须诚实报「未编译/未装配」
//!（测试进程不带 selfupdate manager 注入，wired 分支不触网）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::selfupdate::SelfupdateHandler;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};

fn make_ctx() -> RequestContext {
    let dir = tempfile::tempdir().unwrap();
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
        board: None,
    });
    drop(dir);
    RequestContext {
        session_id: "s-test".to_string(),
        chat_id: "c-test".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[test]
fn module_and_command_manifest() {
    assert_eq!(SelfupdateHandler.module_name(), "selfupdate");
    // 2026-10-21 两段式：新增 download / cancel_download（下载与安装分离）。
    // 2026-10-30 完整性检查：integrity（只读比对已验签基线）。
    assert_eq!(
        SelfupdateHandler.commands(),
        &[
            "check",
            "download",
            "apply",
            "status",
            "cancel_download",
            "integrity"
        ]
    );
}

#[tokio::test]
async fn unknown_command_rejected() {
    let h = SelfupdateHandler;
    let ctx = make_ctx();
    let err = h
        .handle_cmd("nope", None, &ctx)
        .await
        .expect_err("未知命令必须拒");
    assert!(err.contains("selfupdate"), "{err}");
}

#[tokio::test]
async fn status_shape_is_honest_when_unwired() {
    let h = SelfupdateHandler;
    let ctx = make_ctx();
    let out = h
        .handle_cmd("status", None, &ctx)
        .await
        .expect("status 不依赖装配")
        .expect("payload");
    assert_eq!(out["asset"], crate::selfupdate::TARGET_ASSET);
    // 身份槽未注入 = 诚实空值 + journal 相位空；未装配 = wired:false +
    // download 快照 idle + pending/cached_check 空（两段式：审批卡与频率
    // 保险丝已退役，status 不再有 fuse 字段）。
    assert_eq!(out["commit_known"], false);
    assert!(out["journal_phase"].is_null());
    assert_eq!(out["wired"], false);
    // 平台支持契约=apply_supported 的双平台集（2026-10-22 CI Linux 覆盖
    // job 实录：自更新扩 Linux 后此处仍钉单 windows，Linux 上恒假红）。
    assert_eq!(
        out["platform_supported"],
        cfg!(any(target_os = "windows", target_os = "linux"))
    );
    assert_eq!(out["download"]["phase"], "idle");
    assert!(out["pending"].is_null());
    assert!(out["cached_check"].is_null());
    // apply 失败反馈面（2026-10-23 挂死修复）：未装配 = 无在案失败；基准
    // 探测在测试进程 exe 目录恒 false（无 applied_manifest.json）。
    assert!(out["last_apply_error"].is_null());
    assert_eq!(out["sequence_baseline_present"], false);
}

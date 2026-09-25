//! scanner.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 wweb2_tests 同款子模块声明以访问私有项。聚焦仍缺的确定性臂：
//! - `Default`/`module_name`/`commands` 模块接口面（无重复项）
//! - `cmd_enable` 空 state 引擎的 PENDING 注入臂（365-380）：enabled 列表
//!   追加 + `state.install_status = "pending"` 落盘 + 状态投影 enabled=true
//! - `update_db_inner` 的 data_dir 双臂（982-988）：`data_dir` 非空取配置值、
//!   为空回退 `clamav_path/database`——两条路都因 freshclam.exe 缺失立即
//!   Err（`find_executable` 是纯 join，无 PATH 回退，无网络）
//!
//! 结构性豁免（台账 §9.4 延续）：真下载 / 真下载库 / clamd 引擎扫描、
//! spawn 包装层的进度/取消竞态臂、`cfg!(windows)` 的非 Windows 常量假臂。

use super::*;
use crate::api_handlers::AppState;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        event_hub: Arc::new(crate::events::EventHub::new()),
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
        chat_secret_store: Arc::new(()),
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

fn agt_write_cfg(ws: &std::path::Path, engines: serde_json::Value) {
    let dir = ws.join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.scanner.json"),
        serde_json::json!({ "enabled": [], "engines": engines }).to_string(),
    )
    .unwrap();
}

fn agt_read_cfg(ws: &std::path::Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(ws.join("config").join("config.scanner.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

// -----------------------------------------------------------------------
// 模块接口面
// -----------------------------------------------------------------------

#[test]
fn agt_scanner_default_and_module_surface() {
    let h = ScannerHandler::default();
    assert_eq!(h.module_name(), "scanner");
    let cmds = h.commands();
    assert!(cmds.contains(&"config.get"));
    assert!(cmds.contains(&"update_db"));
    assert!(cmds.contains(&"engine.update_config"));
    assert!(cmds.contains(&"cancel"));
    assert_eq!(cmds.len(), 12, "command table size: {}", cmds.len());
    // 声明表不得有重复项（前端命令面板按表渲染）
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "duplicate command in table");
}

// -----------------------------------------------------------------------
// enable：空 state 引擎注入 PENDING 并落盘
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_enable_injects_pending_state_for_empty_engine() {
    let dir = tempfile::tempdir().unwrap();
    let ws_path = dir.path().to_path_buf();
    agt_write_cfg(&ws_path, serde_json::json!({ "clamav": {} }));
    let ctx = agt_make_ctx(&dir);
    let h = ScannerHandler::new();

    // data 缺失 → missing data（分发层）
    let err = h.handle_cmd("enable", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");

    // 未知引擎名
    let err = h
        .handle_cmd("enable", Some(serde_json::json!({ "name": "ghost" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("'ghost' not found"), "err: {err}");

    // 空 state 引擎 → enabled 追加 + PENDING 注入 + 落盘 + 状态投影
    let out = h
        .handle_cmd(
            "enable",
            Some(serde_json::json!({ "name": "clamav" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let engines = out["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0]["name"], "clamav");
    assert_eq!(engines[0]["enabled"], true);
    assert_eq!(engines[0]["state"]["install_status"], "pending");

    let on_disk = agt_read_cfg(&ws_path);
    assert_eq!(on_disk["enabled"][0], "clamav");
    assert_eq!(
        on_disk["engines"]["clamav"]["state"]["install_status"],
        "pending"
    );

    // 已有 install_status 的引擎再 enable：不覆盖 state（幂等臂）
    agt_write_cfg(
        &ws_path,
        serde_json::json!({ "clamav": { "state": { "install_status": "installed" } } }),
    );
    let out = h
        .handle_cmd(
            "enable",
            Some(serde_json::json!({ "name": "clamav" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let engines = out["engines"].as_array().unwrap();
    assert_eq!(
        engines[0]["state"]["install_status"], "installed",
        "non-empty state must be kept"
    );
}

// -----------------------------------------------------------------------
// update_db_inner：data_dir 双臂（freshclam.exe 缺失即 Err，无网络）
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_update_db_inner_prefers_data_dir_and_fails_without_freshclam() {
    let dir = tempfile::tempdir().unwrap();
    let ws_path = dir.path().to_path_buf();
    // clamav_path 指向存在的目录但没有 freshclam.exe → find_executable 纯 join
    // 后 existence check 失败，立即 Err（无 PATH 回退、无网络、无子进程）
    let ws = ws_path.to_string_lossy().to_string();
    let fake_install = dir.path().join("fakeclam");
    std::fs::create_dir_all(&fake_install).unwrap();
    let hub = crate::events::EventHub::new();
    let token = CancellationToken::new();

    // ① data_dir 非空 → 走 engine_cfg.data_dir 臂
    agt_write_cfg(
        &ws_path,
        serde_json::json!({ "clamav": {
            "clamav_path": fake_install.to_string_lossy(),
            "data_dir": dir.path().join("agt-db").to_string_lossy()
        }}),
    );
    let err = update_db_inner(&ws, "clamav", &hub, &token)
        .await
        .unwrap_err();
    assert!(err.contains("freshclam not found"), "err: {err}");

    // ② data_dir 为空/缺省 → 走 clamav_path/database 回退臂
    agt_write_cfg(
        &ws_path,
        serde_json::json!({ "clamav": { "clamav_path": fake_install.to_string_lossy() } }),
    );
    let err2 = update_db_inner(&ws, "clamav", &hub, &token)
        .await
        .unwrap_err();
    assert!(err2.contains("freshclam not found"), "err2: {err2}");
}

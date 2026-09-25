//! security.rs AGT 覆盖率批次（2026-09-25）。与 s10b/f3 两套互补，聚焦仍缺
//! 的确定性臂：
//! - Default 转发 + signature_verify_status 双槽（未注入 = `injected: false`
//!   诚实降级；注入 = 快照六字段透传）
//! - audit / stats 对非 jsonl 目录项的跳过臂（extension 守卫短路）
//! - approvals.clear 的无文件 removed=0 臂 + config 目录创建失败诚实上抛臂
//!   （`<ws>/config` 预置成普通文件 → create_dir_all 必败）

use super::*;
use crate::api_handlers::AppState;
use crate::ws_router::RequestContext;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_ctx(
    ws: &str,
    sig: Option<std::sync::Arc<crate::handlers::signature_status::SignatureVerifyStatus>>,
) -> RequestContext {
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.to_string()),
        home: Some(ws.to_string()),
        state: std::sync::Arc::new(AppState {
            auth_token: String::new(),
            session_count: std::sync::Arc::new(AtomicUsize::new(0)),
            workspace: Some(ws.to_string()),
            home: Some(ws.to_string()),
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: std::sync::Arc::new(parking_lot::Mutex::new("m".to_string())),
            model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
            event_hub: std::sync::Arc::new(crate::events::EventHub::new()),
            running: std::sync::Arc::new(AtomicBool::new(true)),
            session_manager: std::sync::Arc::new(
                crate::session::SessionManager::with_default_timeout(),
            ),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
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
            webhook_rate_limiter: std::sync::Arc::new(
                crate::handlers::workflow::WebhookRateLimiter::new(),
            ),
            #[cfg(not(feature = "workflow"))]
            webhook_rate_limiter: std::sync::Arc::new(()),
            internal_cmd_tx: None,
            estop: None,
            signature_verify: sig,
            cron: None,
            board: None,
        }),
        auth_method: crate::session::AuthMethod::default(),
    }
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    SecurityHandler::new().handle_cmd(cmd, data, ctx).await
}

#[test]
fn agt_default_impl_matches_new() {
    let _ = SecurityHandler::default();
    let _ = SecurityHandler::new();
}

// ---------------------------------------------------------------------------
// signature_verify_status：双槽透传
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_signature_verify_status_injected_and_degraded() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    // 未注入：诚实降级（前端展示「无数据」而非误判成 off）。
    let ctx = agt_ctx(&ws, None);
    let out = run(&ctx, "signature_verify_status", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["injected"], false, "{out}");
    assert!(out.get("mode").is_none(), "{out}");

    // 注入：快照六字段只读透传。
    let ctx = agt_ctx(
        &ws,
        Some(std::sync::Arc::new(
            crate::handlers::signature_status::SignatureVerifyStatus {
                mode: "enforce".to_string(),
                locked: true,
                anchor_fp: Some("af".to_string()),
                last_result: Some("Valid".to_string()),
                key_fp: Some("kp".to_string()),
                detail: "self-check ok".to_string(),
            },
        )),
    );
    let out = run(&ctx, "signature_verify_status", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["injected"], true, "{out}");
    assert_eq!(out["mode"], "enforce");
    assert_eq!(out["locked"], true);
    assert_eq!(out["anchor_fp"], "af");
    assert_eq!(out["last_result"], "Valid");
    assert_eq!(out["key_fp"], "kp");
    assert_eq!(out["detail"], "self-check ok");
}

// ---------------------------------------------------------------------------
// audit / stats：非 jsonl 目录项跳过
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_audit_and_stats_skip_non_jsonl_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let log_dir = dir.path().join("logs").join("security_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    // 合法 jsonl：两行事件（一行 CRITICAL 一行 LOW）。
    std::fs::write(
        log_dir.join("a.jsonl"),
        "{\"event_id\":\"e1\",\"request\":{\"op_type\":\"file_write\",\"danger_level\":\"CRITICAL\",\"target\":\"x\"},\"decision\":\"allowed\",\"timestamp\":\"2026-09-25T00:00:01+08:00\"}\n{\"event_id\":\"e2\",\"request\":{\"op_type\":\"file_read\",\"danger_level\":\"LOW\",\"target\":\"y\"},\"decision\":\"denied\",\"timestamp\":\"2026-09-25T00:00:02+08:00\"}\n",
    )
    .unwrap();
    // 非 jsonl 项：扩展名守卫短路，不读不炸。
    std::fs::write(log_dir.join("notes.txt"), "not audit").unwrap();
    // 无扩展名项同样跳过。
    std::fs::write(log_dir.join("README"), "skip").unwrap();

    let ctx = agt_ctx(&ws, None);
    let out = run(&ctx, "audit", Some(serde_json::json!({ "limit": 10 })))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["entries"].as_array().map(Vec::len), Some(2), "{out}");

    let out = run(&ctx, "stats", None).await.unwrap().unwrap();
    assert_eq!(out["total_events"], 2, "{out}");
    assert_eq!(out["by_level"]["CRITICAL"], 1, "{out}");
    assert_eq!(out["by_level"]["LOW"], 1, "{out}");
}

// ---------------------------------------------------------------------------
// approvals.clear：无文件 removed=0 + config 目录创建失败
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_approvals_clear_missing_file_and_dir_create_fail() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    // ① 无规则文件：removed 0，仍写回空表。
    let ctx = agt_ctx(&ws, None);
    let out = run(&ctx, "approvals.clear", None).await.unwrap().unwrap();
    assert_eq!(out["cleared"], true, "{out}");
    assert_eq!(out["removed"], 0, "{out}");
    let rules = dir.path().join("config").join("approval_rules.json");
    assert_eq!(std::fs::read_to_string(&rules).unwrap(), "[]\n");

    // ② config 位置是普通文件 → create_dir_all 失败 → 诚实上抛。
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("config"), b"not a dir").unwrap();
    let ctx = agt_ctx(&dir2.path().to_string_lossy(), None);
    let err = run(&ctx, "approvals.clear", None).await.unwrap_err();
    assert!(err.contains("failed to create config dir"), "got: {err}");
}

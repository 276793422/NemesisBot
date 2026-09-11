//! P2B（2026-09-12，NB-15 根修配套）：`models.health` WSAPI 测试——
//! 工具健康视图的阈值建议、窗口夹取与账本未装配诚实空三臂。

use super::models::ModelsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_data::DataStore;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 与 m5_session_usage_tests 同款唯一 db 路径（pid + 进程内单调计数器）。
fn temp_db_path() -> std::path::PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nb_web_models_health_{}_{}.db",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// AppState 全量字面量（与 m5_session_usage_tests 同款；chat_secret_store /
/// webhook_rate_limiter 按 workflow feature 双臂）。
fn make_ctx(dir: &tempfile::TempDir, data_store: Option<Arc<DataStore>>) -> RequestContext {
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
        data_store,
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
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// health 恒 Ok(Some)——unwrap 成 Value 的小助手。
async fn health_call(
    handler: &ModelsHandler,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    handler
        .handle_cmd("health", data, ctx)
        .await
        .expect("models.health 不得报错")
        .expect("models.health 恒返回 Some")
}

/// 样本足够（≥10 次调用）且失败率 ≥20% → 带 tier 校准建议；样本不足
/// → 只报事实不出结论。
#[tokio::test]
async fn models_health_hint_threshold_and_small_sample() {
    let dir = tempfile::TempDir::new().unwrap();
    let ds = Arc::new(DataStore::open(&temp_db_path()).unwrap());
    // m-hi：12 次调用 3 次失败（25% ≥ 20%）→ 必有 hint。
    for i in 0..12 {
        ds.record_tool_validation("m-hi", i % 4 == 0).unwrap();
    }
    // m-lo：3 次调用 1 次失败（33% 但样本 < 10）→ 无 hint。
    ds.record_tool_validation("m-lo", true).unwrap();
    ds.record_tool_validation("m-lo", false).unwrap();
    ds.record_tool_validation("m-lo", false).unwrap();

    let handler = ModelsHandler::new();
    let ctx = make_ctx(&dir, Some(ds));
    let out = health_call(&handler, &ctx, None).await;
    assert_eq!(out["days"], 7, "默认窗口 7 天");
    let models = out["models"].as_array().unwrap();
    let hi = models
        .iter()
        .find(|m| m["model"] == "m-hi")
        .expect("m-hi 必须在列");
    assert_eq!(hi["tool_calls"], 12);
    assert_eq!(hi["validation_failures"], 3);
    let hint = hi["hint"].as_str().expect("25% 失败率必须给建议");
    assert!(
        hint.contains("probe") && hint.contains("set-tier"),
        "{hint}"
    );
    let lo = models
        .iter()
        .find(|m| m["model"] == "m-lo")
        .expect("m-lo 必须在列");
    assert!(
        lo["hint"].is_null(),
        "样本不足（3 次）不得出结论: {:?}",
        lo["hint"]
    );
}

/// data.days 窗口夹取（1~90）；账本未装配 = 空列表 + 诚实 note。
#[tokio::test]
async fn models_health_days_clamp_and_missing_ledger() {
    let dir = tempfile::TempDir::new().unwrap();
    let handler = ModelsHandler::new();

    let ctx = make_ctx(
        &dir,
        Some(Arc::new(DataStore::open(&temp_db_path()).unwrap())),
    );
    let out = health_call(&handler, &ctx, Some(serde_json::json!({ "days": 0 }))).await;
    assert_eq!(out["days"], 1, "days=0 夹取为 1");
    let out = health_call(&handler, &ctx, Some(serde_json::json!({ "days": 500 }))).await;
    assert_eq!(out["days"], 90, "days=500 夹取为 90");

    let ctx_no_ds = make_ctx(&dir, None);
    let out = health_call(&handler, &ctx_no_ds, None).await;
    assert!(out["models"].as_array().unwrap().is_empty());
    assert!(out["note"].as_str().is_some(), "必须诚实注记账本未装配");
}

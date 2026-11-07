//! 回归钉：`models.add` 模型 identifier 形态校验——此前
//! WSAPI add 零校验（连 CLI 的 `contains('/')` 闸都没有），`../../etc/passwd`
//! 等穿越形态可落盘。校验走 `validate_model_identifier` 单一真相源（与 CLI
//! `model add` 同源），且在触碰 config.json **之前**执行（非法形态错误优先于
//! config 缺失错误）。make_ctx 与 models_health_tests 同款。

use super::models::ModelsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
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

fn add_body(model: &str) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "name": "alias-x",
        "model": model,
        "key": "k",
    }))
}

#[tokio::test]
async fn add_rejects_traversal_identifier_before_touching_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // config.json 不存在——若校验缺失，错误会是 config 读取失败而非形态拒绝；
    // 断言错误消息来自 identifier 校验（'..' 段），即校验先于 config 触达。
    let err = h
        .handle_cmd("add", add_body("../../etc/passwd"), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("'.'/'..'"),
        "穿越形态必须被 identifier 校验拒绝，got: {err}"
    );
}

#[tokio::test]
async fn add_rejects_backslash_traversal_and_empty_segment() {
    let tmp = tempfile::TempDir::new().unwrap();
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 宽松档：裸名合法（dashboard 既有契约），但穿越段/反斜杠/空段仍拒——
    // 含裸 `..`（无斜杠的穿越形态）。
    for bad in [r"openai\gpt-4o", "..", "../etc", "openai/", "openai//gpt"] {
        let err = h
            .handle_cmd("add", add_body(bad), &ctx)
            .await
            .err()
            .unwrap_or_else(|| panic!("'{bad}' 应被拒绝"));
        assert!(
            err.contains("Invalid model identifier"),
            "'{bad}' 错误应来自形态校验，got: {err}"
        );
    }
}

#[tokio::test]
async fn add_accepts_valid_and_bare_identifiers_then_reports_missing_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 合法形态（vendor/model + 裸名）放行校验闸 → 错误推进到 config 读取
    // （无 model_list / 无文件）。裸名 = dashboard 既有契约（glm-5.3-flash
    // 先例，resolve_model_config BUG-b 无前缀条目生产支持）。
    for good in ["openai/gpt-4o", "glm-5.3-flash"] {
        let err = h.handle_cmd("add", add_body(good), &ctx).await.unwrap_err();
        assert!(
            !err.contains("Invalid model identifier"),
            "合法形态 '{good}' 不应被形态校验拒绝，got: {err}"
        );
        assert!(
            err.contains("config.json") || err.contains("model_list"),
            "合法形态 '{good}' 应推进到 config 触达，got: {err}"
        );
    }
}

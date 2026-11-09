//! config.save round-trip 校验（C-2）测试：未建模键 loud 拒绝、空值豁免不误报、
//! 合法整档保存通过。helper 直测覆盖路径配对语义。
//! 夹具镜像 config_s10b_tests.rs（AppState 无 Default，字段逐个显式）。

use super::collect_dropped_paths;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::config::ConfigHandler;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_state(ws: Option<String>) -> Arc<AppState> {
    Arc::new(AppState {
        cors_manager: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: ws.clone(),
        home: ws.clone(),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
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
    })
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let config = nemesis_config::Config::default();
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    RequestContext {
        session_id: "s-c2".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state: make_state(Some(dir.path().to_string_lossy().to_string())),
        auth_method: crate::session::AuthMethod::default(),
    }
}

// ---- helper 直测 ----

#[test]
fn dropped_paths_unmodeled_key_with_payload_collected() {
    let inc = serde_json::json!({ "agents": { "temperature": 0.7 }, "custom_thing": { "a": 1 } });
    let re = serde_json::json!({ "agents": { "temperature": 0.7 } });
    assert_eq!(collect_dropped_paths(&inc, &re), vec!["custom_thing"]);
}

#[test]
fn dropped_paths_empty_values_exempt() {
    // 空值叶子（null/false/空串/空数组/空对象）整棵消失 = skip_serializing_if
    // 语义，不算丢键。
    let inc = serde_json::json!({
        "custom_a": null,
        "custom_b": false,
        "custom_c": "",
        "custom_d": [],
        "custom_e": {},
        "custom_f": { "nested": null, "deep": { "deeper": false } },
        "agents": { "temperature": 0.7 }
    });
    let re = serde_json::json!({ "agents": { "temperature": 0.7 } });
    assert!(collect_dropped_paths(&inc, &re).is_empty());
}

#[test]
fn dropped_paths_nested_key_and_array_index() {
    let inc = serde_json::json!({
        "model_list": [
            { "model": "m1", "api_key": "k1" },
            { "model": "m2", "api_key": "k2", "extra_field": "kept-in-model" }
        ]
    });
    let re = serde_json::json!({
        "model_list": [
            { "model": "m1", "api_key": "k1" }
        ]
    });
    assert_eq!(collect_dropped_paths(&inc, &re), vec!["model_list.1"]);
}

#[test]
fn dropped_paths_normalized_value_collected() {
    // 值被 typed 归一（0.7 → 不同值）= 信息丢失，同样 loud。
    let inc = serde_json::json!({ "agents": { "temperature": 0.7 } });
    let re = serde_json::json!({ "agents": { "temperature": 0.5 } });
    assert_eq!(collect_dropped_paths(&inc, &re), vec!["agents.temperature"]);
}

// ---- handler 级 ----

async fn get_config(handler: &ConfigHandler, ctx: &RequestContext) -> serde_json::Value {
    handler
        .handle_cmd("get", None, ctx)
        .await
        .expect("config.get should succeed")
        .expect("config.get should return data")
}

#[tokio::test]
async fn config_save_unmodeled_key_loud_rejected_and_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = ConfigHandler::new();

    let mut payload = get_config(&handler, &ctx).await;
    payload["custom_unknown_key"] = serde_json::json!({ "important": "value" });

    let err = handler
        .handle_cmd("save", Some(payload), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("silently dropped"), "{err}");
    assert!(err.contains("custom_unknown_key"), "{err}");

    // 闭包 Err 不落盘：盘上没有这个键。
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert!(on_disk.get("custom_unknown_key").is_none());
}

#[tokio::test]
async fn config_save_full_get_response_roundtrip_passes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = ConfigHandler::new();

    // UI raw 编辑的真实形态：get 掩码视图原样存回 = 合法，不误报。
    let payload = get_config(&handler, &ctx).await;
    let result = handler.handle_cmd("save", Some(payload), &ctx).await;
    assert!(
        result.is_ok(),
        "full get→save roundtrip should pass: {result:?}"
    );

    let saved = handler.handle_cmd("get", None, &ctx).await;
    assert!(saved.is_ok());
}

#[tokio::test]
async fn config_save_empty_valued_fields_pass() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = ConfigHandler::new();

    // 建模字段的空值（skip_serializing_if 空段）合法保存，不触发豁免误报。
    let mut payload = get_config(&handler, &ctx).await;
    payload["agents"]["restrict_to_workspace"] = serde_json::Value::Bool(false);
    payload["gateway"]["host"] = serde_json::Value::String(String::new());

    let result = handler.handle_cmd("save", Some(payload), &ctx).await;
    assert!(
        result.is_ok(),
        "empty-valued modeled fields should pass: {result:?}"
    );
}

// ---- config.set_field（f64 数值归一 + None 段自动装配）----

#[tokio::test]
async fn config_set_field_f64_integral_normalization_passes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = ConfigHandler::new();

    // f64 字段收整数（wire 形态 JS Number 恒发整数）：typed reparse 把
    // PosInt(1) 合法归一成 Float(1.0)——数值语义相同，不得误判
    // round-trip 失败。connectors 段缺席（None → null/缺键）时 set_json_path
    // 自动装配中间对象，再 serde(default) 补齐兄弟字段。
    handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({
                "path": "connectors.finance.max_order_qty",
                "value": 1
            })),
            &ctx,
        )
        .await
        .expect("integral f64 set_field must pass")
        .expect("set_field returns payload");

    let cfg = get_config(&handler, &ctx).await;
    assert_eq!(
        cfg["connectors"]["finance"]["max_order_qty"], 1.0,
        "value must persist (as f64 1.0)"
    );
    // 兄弟字段走 serde default 补齐，不因自动装配丢失。
    assert_eq!(cfg["connectors"]["finance"]["max_orders_per_day"], 0);
    assert_eq!(
        cfg["connectors"]["enabled"], true,
        "section default applies"
    );
}

#[tokio::test]
async fn config_set_field_ui_skins_bool_passes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = ConfigHandler::new();

    // ui.skins.require_signed（皮肤面板读改面）：ui 段缺席时同样自动装配。
    handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({
                "path": "ui.skins.require_signed",
                "value": true
            })),
            &ctx,
        )
        .await
        .expect("ui.skins set_field must pass")
        .expect("set_field returns payload");

    let cfg = get_config(&handler, &ctx).await;
    assert_eq!(cfg["ui"]["skins"]["require_signed"], true);
    // 同段兄弟字段保留缺省。
    assert_eq!(cfg["ui"]["skins"]["allow_scripts"], false);
}

#[tokio::test]
async fn config_set_field_unknown_path_still_loud_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = ConfigHandler::new();

    // 数值归一只放行语义相同的数字；未建模键照旧 loud 拒绝（typed reparse
    // 丢弃 → round-trip 缺失 → unknown config field）。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({
                "path": "nonexistent_section.some_key",
                "value": 1
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown config field"), "{err}");
}

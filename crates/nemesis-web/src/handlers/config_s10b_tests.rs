//! S10b (quality-hardening goal 冲刺, web 批次 2): ConfigHandler error arms and
//! CORS stubs the gated `mod tests` skips — missing config.json load failure,
//! invalid save payload, empty-path / invalid-result set_field, and the four
//! "CORS manager not connected" stub commands.

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
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    })
}

fn make_ctx(dir: &tempfile::TempDir, with_config: bool) -> RequestContext {
    if with_config {
        let config = nemesis_config::Config::default();
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }
    let ws = dir.path().to_string_lossy().to_string();
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state: make_state(Some(dir.path().to_string_lossy().to_string())),
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn config_get_with_malformed_config_file_errors() {
    // Missing config.json falls back to defaults (Ok) — a present-but-broken
    // file is what trips the load error arm.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{definitely not json").unwrap();
    let ctx = make_ctx(&dir, false);
    let handler = ConfigHandler::new();
    let err = handler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("failed to load config"), "{err}");
}

#[tokio::test]
async fn config_save_rejects_wrong_typed_payload() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();
    let err = handler
        .handle_cmd(
            "save",
            Some(serde_json::json!({ "model_list": "not-an-array" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config data"), "{err}");
}

#[tokio::test]
async fn config_set_field_empty_path_and_invalid_result_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let err = handler
        .handle_cmd("set_field", Some(serde_json::json!({ "path": "" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "empty path");

    // model_list must stay an array — a string breaks re-parse.
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list", "value": "oops" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config after field update"), "{err}");
}

#[tokio::test]
async fn config_cors_stubs_report_not_connected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let list = handler.handle_cmd("cors.list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["origins"].as_array().unwrap().len(), 0);
    assert!(list["message"].as_str().unwrap().contains("not connected"));

    let added = handler
        .handle_cmd("cors.add", Some(serde_json::json!({ "origin": "https://x.com" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(added["added"], false);

    let removed = handler
        .handle_cmd("cors.remove", Some(serde_json::json!({ "origin": "https://x.com" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(removed["removed"], false);

    let toggled = handler
        .handle_cmd("cors.toggle", Some(serde_json::json!({ "enabled": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(toggled["toggled"], false);
}

// ---------------------------------------------------------------------------
// G6：agents.tool_doc_folding 开关 —— set_field 可写（typed round-trip），
// config.get 回读，盘上真实落盘；错型值 loud 拒绝。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_tool_doc_folding_toggle_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    // 默认关。
    let get = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
    assert_eq!(get["agents"]["tool_doc_folding"]["enabled"], false);

    // 开 → updated + get 回读 true。
    let out = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled", "value": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);
    let get = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
    assert_eq!(get["agents"]["tool_doc_folding"]["enabled"], true);

    // 盘上真实落盘（typed 序列化，不是只在内存）。
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], true);
    // expand_top_n 保持 serde 默认 8。
    assert_eq!(raw["agents"]["tool_doc_folding"]["expand_top_n"], 8);

    // 关回去。
    handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled", "value": false })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], false);
}

#[tokio::test]
async fn config_tool_doc_folding_rejects_wrong_typed_value() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    // enabled 必须是 bool —— 字符串 "on" 走 typed 反序列化失败，loud 拒绝。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled", "value": "on" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config after field update"), "{err}");

    // 拒绝后盘上仍是 false（默认值），没有被半写。
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], false);
}

// R1 真机验收发现：未知键曾走 generic set_field 静默丢弃（serde 忽略未知字段）
// 却谎报 updated:true —— 现按 G6「未知键必须 loud 拒绝」要求 round-trip 校验。
#[tokio::test]
async fn config_set_field_unknown_key_loud_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.nonexistent_key_xyz", "value": 1 })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown config field"), "{err}");

    // 盘上没有被半写（不存在中间对象被静默创建）。
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert!(raw["agents"].get("nonexistent_key_xyz").is_none());

    // 深层未知路径同样拒绝（set_json_path 曾自动创建中间对象）。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.no_such_section.deeper", "value": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown config field"), "{err}");
}

// ---------------------------------------------------------------------------
// 数组下标路径（model_list.<i>.<field>）：set_json_path / json_path_get 必须
// 对称支持数字段（此前 get 对数组数字段返回 None → round-trip 误报
// "unknown config field"，set 本身也是静默 no-op）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_set_field_supports_array_index_paths() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    // 播种一个带条目的 model_list（Config 全字段 serde(default)，最小 JSON 可解析）。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({ "model_list": [ { "model_name": "test/m1" } ] }).to_string(),
    )
    .unwrap();
    let handler = ConfigHandler::new();

    // 数组下标路径可写且 round-trip 通过（不再误报 unknown config field）。
    let out = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list.0.model_name", "value": "test/m2" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);

    // get 回读 + 盘上真实落盘。
    let get = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
    assert_eq!(get["model_list"][0]["model_name"], "test/m2");
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw["model_list"][0]["model_name"], "test/m2");

    // 越界下标 loud 报错（无 append 语义），盘上没有被半写。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list.5.model_name", "value": "x" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("array index out of bounds"), "{err}");

    // 数组上的非数字段 loud 报错（不是静默 no-op）。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list.zero", "value": "x" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not a valid array index"), "{err}");
}

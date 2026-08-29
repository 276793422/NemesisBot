//! Tests for `PluginsHandler`（插件状态总览，2026-08-29）。
//! 直连 workspace 方法 + **dispatch 级**（handle_cmd 裸名——commands.list
//! 事故教训：match 臂命令名错误只有 dispatch 层能抓住）。

use super::*;

fn ws(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
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

#[test]
fn plugins_list_reports_both_known_plugins_without_error() {
    let dir = tempfile::tempdir().unwrap();
    let r = PluginsHandler::new().plugins_list(&ws(&dir)).unwrap();

    let arr = r["plugins"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "onnx + ui");
    assert_eq!(arr[0]["id"], "plugin_onnx");
    assert_eq!(arr[1]["id"], "plugin_ui");

    // 测试进程 exe 旁没有 plugins/ → found:false 是合法状态（不报错）。
    for p in arr {
        assert_eq!(p["found"], false, "test env has no plugins dir: {p}");
    }

    // feature 状态数组：7 项子系统和每项 enabled 为 bool。
    let features = r["features"].as_array().unwrap();
    assert_eq!(features.len(), 7);
    for f in features {
        assert!(f["enabled"].is_boolean(), "{f}");
    }
}

#[test]
fn plugins_list_onnx_detail_reflects_model_readiness() {
    let dir = tempfile::tempdir().unwrap();
    // 播种 embedding 配置（enabled + medium 档模型就绪）。
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.enhanced_memory.json"),
        serde_json::json!({
            "enabled": true,
            "active": "medium",
            "models": {
                "medium": {
                    "name": "all-MiniLM-L6-v2",
                    "dimension": 384,
                    "model_url": "https://example.invalid/model.onnx",
                    "tokenizer_url": "https://example.invalid/tokenizer.json",
                    "local_model_path": "",
                    "local_tokenizer_path": ""
                }
            },
            "auto_inject": false,
            "auto_inject_top_k": 3
        })
        .to_string(),
    )
    .unwrap();
    // 造一个假模型文件让 model_ready 为真。
    let model_dir = dir
        .path()
        .join("tools/memory/data/embedding/all-MiniLM-L6-v2");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join("model.onnx"), b"fake").unwrap();

    #[cfg(feature = "memory")]
    {
        let r = PluginsHandler::new().plugins_list(&ws(&dir)).unwrap();
        let onnx = &r["plugins"][0];
        assert_eq!(onnx["detail"]["enhanced_memory_enabled"], true);
        assert_eq!(onnx["detail"]["active_tier"], "medium");
        assert_eq!(onnx["detail"]["model_ready"], true);
    }
    #[cfg(not(feature = "memory"))]
    {
        let _ = ws(&dir);
    }
}

#[tokio::test]
async fn dispatch_list_via_bare_cmd_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PluginsHandler::new();

    // 裸名 "list"（不是 "plugins.list"）——commands.list 事故的 dispatch 级钉。
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert!(r["plugins"].as_array().unwrap().len() == 2);

    // 未知子命令 → loud 错误。
    let err = h.handle_cmd("bogus", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: plugins.bogus"), "{err}");
}

#[tokio::test]
async fn dispatch_set_metrics_enabled_flips_and_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PluginsHandler::new();

    // 关。
    let out = h
        .handle_cmd(
            "set_metrics_enabled",
            Some(serde_json::json!({ "enabled": false })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["enabled"], serde_json::json!(false));

    // 开。
    let out = h
        .handle_cmd(
            "set_metrics_enabled",
            Some(serde_json::json!({ "enabled": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["enabled"], serde_json::json!(true));

    // 缺 enabled 字段 → loud。
    let err = h
        .handle_cmd("set_metrics_enabled", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("enabled"), "{err}");
}

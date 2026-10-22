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
fn plugins_list_reports_full_native_catalog_without_error() {
    let dir = tempfile::tempdir().unwrap();
    let r = PluginsHandler::new().plugins_list(&ws(&dir)).unwrap();

    let arr = r["plugins"].as_array().unwrap();
    // 目录 = nemesis-config NativePluginId::ALL 五成员（2026-10-23 补登记
    // plugin_vm——此前 VM 显示通道被误列「未识别的库」的花名册缺口根修）。
    let ids: Vec<&str> = arr.iter().filter_map(|p| p["id"].as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "plugin_onnx",
            "plugin_ui",
            "eval_monitor_dll",
            "plugin_wasm",
            "plugin_vm"
        ]
    );

    // 测试进程 exe 旁没有 plugins/ → found:false 是合法状态（不报错）。
    for p in arr {
        assert_eq!(p["found"], false, "test env has no plugins dir: {p}");
        // 开关缺省 = 启用（向后兼容语义）；生效语义标记在位。
        assert_eq!(p["enabled"], true, "{p}");
        assert_eq!(p["immediate_effect"], p["id"] == "eval_monitor_dll", "{p}");
        // probe 未注入 = dormant 诚实降级。
        assert_eq!(p["signature"]["mode"], "dormant", "{p}");
    }
    // 未知库清单字段在位（测试环境无 plugins/ 目录 = 空表）。
    assert_eq!(r["unrecognized_libraries"], serde_json::json!([]));

    // feature 状态数组已迁出（system.features / About 页构建形态 tab 消费）——
    // 这里钉「不再返回」，防止旧字段悄悄回潮。
    assert!(
        r.get("features").is_none(),
        "plugins.list 不再携带 features（迁至 system.features）：{r}"
    );
}

#[tokio::test]
async fn native_set_enabled_persists_and_list_reflects() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PluginsHandler::new();

    // 关掉 plugin_ui。
    let out = h
        .handle_cmd(
            "native_set_enabled",
            Some(serde_json::json!({ "id": "plugin_ui", "enabled": false })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["enabled"], serde_json::json!(false));
    assert_eq!(out["immediate_effect"], serde_json::json!(false));

    // 清单反映开关（其他条目仍缺省启用）。
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    for p in r["plugins"].as_array().unwrap() {
        let expected = p["id"] != "plugin_ui";
        assert_eq!(p["enabled"], expected, "{p}");
    }

    // 未知 id → loud；缺字段 → loud。
    let err = h
        .handle_cmd(
            "native_set_enabled",
            Some(serde_json::json!({ "id": "nope", "enabled": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("未知原生插件 id"), "{err}");
    let err = h
        .handle_cmd("native_set_enabled", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("id"), "{err}");
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
    assert!(r["plugins"].as_array().unwrap().len() == 5);

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

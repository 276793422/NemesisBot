//! models.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 tests / s10b_tests / models_health_tests 互补，聚焦仍缺的确定性臂：
//! - `Default` impl 与命令表
//! - `proxy_overview` 全身（模型行 + 环境变量代理回显 + lane 支持表 + notes）
//! - `update_field` 的 proxy 非法前缀拒绝臂
//! - `update_field` 改默认模型 proxy/protocol 时 entry_is_default 的
//!   model 串命中臂与别名命中臂（热切联动）
//!
//! 结构性豁免（见报告）：`load_config`/`write_raw_config` 的进程级
//! global ConfigStore 分支（测试进程不装配 global store）、DISABLED
//! `save_config`（allow(dead_code)）、set_default 的 projects_bridge
//! 全局桥分支（未装配）、`catalog_update`（spawn 测试二进制自身 →
//! libtest 参数被当过滤器，递归风暴，与 sandbox run_cli_subcmd 同豁免）。

use super::*;

fn agt_write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn agt_read_config_raw(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

fn agt_home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

fn agt_make_ctx(dir: &tempfile::TempDir) -> crate::ws_router::RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
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
    crate::ws_router::RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[test]
fn agt_default_impl_and_command_table() {
    let h = ModelsHandler::default();
    assert_eq!(h.module_name(), "models");
    let cmds = h.commands();
    assert!(cmds.contains(&"list"));
    assert!(cmds.contains(&"proxy_overview"));
    assert!(cmds.contains(&"health"));
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "duplicate command in table");
}

#[tokio::test]
async fn agt_proxy_overview_shape() {
    let dir = tempfile::tempdir().unwrap();
    agt_write_config(
        dir.path(),
        r#"{
            "agents": { "defaults": { "llm": "m2" } },
            "model_list": [
                { "model_name": "m1", "model": "qwen/qwen3-30b", "api_key": "sk", "protocol": "anthropic", "proxy": "http://127.0.0.1:8080" },
                { "model_name": "m2", "model": "test/testai-1.1" }
            ]
        }"#,
    );
    let ctx = agt_make_ctx(&dir);
    let out = ModelsHandler::new()
        .handle_cmd("proxy_overview", None, &ctx)
        .await
        .unwrap()
        .unwrap();

    let models = out["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    // m1：显式 protocol/proxy 回显；非默认
    assert_eq!(models[0]["model_name"], "m1");
    assert_eq!(models[0]["protocol"], "anthropic");
    assert_eq!(models[0]["proxy"], "http://127.0.0.1:8080");
    assert_eq!(models[0]["is_default"], false);
    // m2：空 protocol/proxy = 自动/直连；是 agents.defaults.llm 默认
    assert_eq!(models[1]["protocol"], "");
    assert_eq!(models[1]["proxy"], "");
    assert_eq!(models[1]["is_default"], true);

    // 环境变量代理键四种都回显（测试进程通常未设置 → 空串占位）
    let env = &out["env"];
    for key in ["http_proxy", "https_proxy", "all_proxy", "no_proxy"] {
        assert!(env.get(key).is_some(), "env.{key} missing: {out}");
        assert!(env[key].is_string());
    }
    // lane 支持表：3 个 HTTP lane 全支持 per-model proxy，CLI lane 不支持
    let lanes = out["lane_support"].as_array().unwrap();
    assert_eq!(lanes.len(), 4);
    assert_eq!(lanes[0]["per_model_proxy"], true);
    assert_eq!(lanes[3]["per_model_proxy"], false);
    assert!(lanes[3]["note"].as_str().unwrap().contains("HTTPS_PROXY"));
    assert_eq!(out["notes"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn agt_update_field_proxy_rejects_bad_prefix() {
    let dir = tempfile::tempdir().unwrap();
    agt_write_config(
        dir.path(),
        r#"{ "model_list": [ { "model_name": "m1", "model": "test/testai-1.1" } ] }"#,
    );
    let ctx = agt_make_ctx(&dir);
    let err = ModelsHandler::new()
        .update_field(
            &agt_home_str(&dir),
            &serde_json::json!({ "name": "m1", "field": "proxy", "value": "ftp://proxy:8080" }),
            &ctx,
        )
        .unwrap_err();
    assert!(err.contains("proxy must start with"), "err: {err}");
    // 拒绝后 config 不变
    let cfg = agt_read_config_raw(dir.path());
    assert!(cfg["model_list"][0].get("proxy").is_none());
}

/// entry_is_default 的两个非 model_name 命中臂（719-720）：
/// agents.defaults.llm = vendor/model 串 / 别名（末段）。
#[tokio::test]
async fn agt_update_field_proxy_default_detection_by_model_and_alias() {
    let h = ModelsHandler::new();

    // ① 默认 = vendor/model 串（719 臂）
    let dir = tempfile::tempdir().unwrap();
    agt_write_config(
        dir.path(),
        r#"{
            "agents": { "defaults": { "llm": "zhipu/glm-4.7-flash" } },
            "model_list": [ { "model_name": "m1", "model": "zhipu/glm-4.7-flash", "api_key": "sk" } ]
        }"#,
    );
    let ctx = agt_make_ctx(&dir);
    let out = h
        .update_field(
            &agt_home_str(&dir),
            &serde_json::json!({ "name": "m1", "field": "proxy", "value": "http://127.0.0.1:9999" }),
            &ctx,
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(
        agt_read_config_raw(dir.path())["model_list"][0]["proxy"],
        "http://127.0.0.1:9999"
    );

    // ② 默认 = 别名（model 末段，720 臂）；再用空串清除 proxy（直连）
    let dir2 = tempfile::tempdir().unwrap();
    agt_write_config(
        dir2.path(),
        r#"{
            "agents": { "defaults": { "llm": "glm-4.7-flash" } },
            "model_list": [ { "model_name": "m1", "model": "zhipu/glm-4.7-flash", "api_key": "sk" } ]
        }"#,
    );
    let ctx2 = agt_make_ctx(&dir2);
    let out2 = h
        .update_field(
            &agt_home_str(&dir2),
            &serde_json::json!({ "name": "m1", "field": "proxy", "value": "" }),
            &ctx2,
        )
        .unwrap()
        .unwrap();
    assert_eq!(out2["value"], "");
    let cfg2 = agt_read_config_raw(dir2.path());
    assert_eq!(cfg2["model_list"][0]["proxy"], "");

    // ③ 对照：非默认模型改 proxy → 不触发热切，仅落盘
    let dir3 = tempfile::tempdir().unwrap();
    agt_write_config(
        dir3.path(),
        r#"{
            "agents": { "defaults": { "llm": "other" } },
            "model_list": [ { "model_name": "m1", "model": "zhipu/glm-4.7-flash", "api_key": "sk" } ]
        }"#,
    );
    let ctx3 = agt_make_ctx(&dir3);
    h.update_field(
        &agt_home_str(&dir3),
        &serde_json::json!({ "name": "m1", "field": "proxy", "value": "socks5://127.0.0.1:1080" }),
        &ctx3,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        agt_read_config_raw(dir3.path())["model_list"][0]["proxy"],
        "socks5://127.0.0.1:1080"
    );
}

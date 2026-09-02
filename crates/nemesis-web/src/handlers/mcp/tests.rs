//! MCP handler 补测（Phase 3 覆盖率，2026-08-25）。
//!
//! 既有 `handlers/tests.rs` 已盖 server.add/delete、config.get/save、
//! unknown cmd。这里补两块显示/更新语义：
//! ① `servers` 列表的 legacy 归一化显示（transport_type 空 → "stdio"、
//!    url 空 → 显示 command）——`McpServerConfig::normalize` 只在 add 时跑，
//!    手写的存量配置文件里可以没有这两个字段，展示层必须兜底；
//! ② `server.update` 的全部可选字段 patch 臂（transport_type/description/
//!    headers/provider_name/provider_url/tags——旧清单只盖了 url/args/env/
//!    timeout）。

use super::McpHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 手写一份存量形态的 config.mcp.json（transport_type/url 缺省）。
fn seed_mcp_config(dir: &tempfile::TempDir, servers: serde_json::Value) {
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.mcp.json"),
        serde_json::json!({ "enabled": true, "servers": servers }).to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn servers_display_normalizes_legacy_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // 存量配置：只有 command，transport_type/url 全空。
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "legacy", "command": "run-server.sh" }]),
    );

    let h = McpHandler::new();
    let out = h.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
    let servers = out["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    // 展示层兜底：空 transport_type 显示 "stdio"，空 url 显示 command。
    assert_eq!(servers[0]["transport_type"], "stdio");
    assert_eq!(servers[0]["url"], "run-server.sh");
}

#[tokio::test]
async fn server_update_patches_all_optional_fields_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "s1", "url": "http://old" }]),
    );

    let h = McpHandler::new();
    let out = h
        .handle_cmd(
            "server.update",
            Some(serde_json::json!({
                "name": "s1",
                "transport_type": "http",
                "description": "patched desc",
                "headers": ["Authorization: b"],
                "provider_name": "prov",
                "provider_url": "https://prov.example",
                "tags": ["a", "b"],
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);

    // 落盘回读（config.get）验证全部字段持久化。
    let cfg = h
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let s = &cfg["servers"][0];
    assert_eq!(s["transport_type"], "http");
    assert_eq!(s["url"], "http://old", "未 patch 的字段必须保持原值");
    assert_eq!(s["description"], "patched desc");
    assert_eq!(s["headers"], serde_json::json!(["Authorization: b"]));
    assert_eq!(s["provider_name"], "prov");
    assert_eq!(s["provider_url"], "https://prov.example");
    assert_eq!(s["tags"], serde_json::json!(["a", "b"]));
}

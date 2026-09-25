//! 凭据回显脱敏批次（2026-09-25；vault 方案 0.4.7 遗留收尾）测试。
//!
//! 三件事：① 共享助手语义（mask_secret_entry / restore_masked_entries /
//! restore_masked_named_fields）；② WSAPI 回显面脱敏（mcp servers/config.get、
//! cluster config.get）；③ 保存路径防掩码回写（update/config_save 还原存量
//! 原值，add/config_save 无原值 loud 拒绝——绝不把 `****` 当真值落盘）。

use super::*;
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
        signature_verify: None,
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

// -- ① 共享助手语义 ----------------------------------------------------------

#[test]
fn mask_secret_entry_masks_sensitive_value_part_only() {
    // 敏感键（authorization/token/key/secret/password/cookie/credential 子串）：
    // 值部脱敏、键部与分隔符保留。
    assert_eq!(
        mask_secret_entry("Authorization: Bearer sk-1234567890abcdef"),
        "Authorization: Bear****cdef"
    );
    assert_eq!(
        mask_secret_entry("OPENAI_API_KEY=sk-abcdefgh12345678"),
        "OPENAI_API_KEY=sk-a****5678"
    );
    // 短值（≤8）整体打码。
    assert_eq!(mask_secret_entry("X-Api-Key: short"), "X-Api-Key: ****");
    // 非敏感键原样通过。
    assert_eq!(
        mask_secret_entry("Content-Type: application/json"),
        "Content-Type: application/json"
    );
    assert_eq!(mask_secret_entry("HOME=/usr/local"), "HOME=/usr/local");
    // 无分隔符的敏感裸词整体脱敏。
    assert_eq!(mask_secret_entry("supersecret"), "supe****cret");
}

#[test]
fn restore_masked_entries_restores_by_key_and_rejects_unmatched() {
    let existing = vec![
        "Authorization: Bearer real-secret-value".to_string(),
        "X-Trace: 1".to_string(),
    ];
    let mut incoming = vec![
        "Authorization: Bear****alue".to_string(),
        "X-Trace: 1".to_string(),
    ];
    restore_masked_entries(&mut incoming, &existing).unwrap();
    assert_eq!(incoming[0], "Authorization: Bearer real-secret-value");
    assert_eq!(incoming[1], "X-Trace: 1");

    // 无同键原值 → loud 拒绝（绝不能落盘掩码）。
    let mut orphan = vec!["X-Api-Key: ****".to_string()];
    assert!(restore_masked_entries(&mut orphan, &existing).is_err());
}

#[test]
fn restore_masked_named_fields_restores_and_rejects() {
    let existing = serde_json::json!({
        "token": "real-cluster-token-value",
        "nested": { "password": "hunter2" },
        "plain": "visible"
    });
    let mut incoming = serde_json::json!({
        "token": "real****alue",
        "nested": { "password": "****" },
        "plain": "visible",
        "other": "untouched"
    });
    restore_masked_named_fields(&mut incoming, &existing).unwrap();
    assert_eq!(incoming["token"], "real-cluster-token-value");
    assert_eq!(incoming["nested"]["password"], "hunter2");
    assert_eq!(incoming["other"], "untouched");

    // 存量缺失 / 存量同为掩码 → 都 loud 拒绝。
    let mut no_orig = serde_json::json!({ "api_key": "sk-1****cdef" });
    assert!(restore_masked_named_fields(&mut no_orig, &serde_json::json!({})).is_err());
    let mut masked_orig = serde_json::json!({ "api_key": "sk-1****cdef" });
    let existing_masked = serde_json::json!({ "api_key": "sk-1****cdef" });
    assert!(restore_masked_named_fields(&mut masked_orig, &existing_masked).is_err());
}

// -- ② + ③ MCP 面 ------------------------------------------------------------

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
async fn mcp_servers_echo_masks_headers_and_env() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    seed_mcp_config(
        &dir,
        serde_json::json!([{
            "name": "s1",
            "url": "http://x",
            "headers": ["Authorization: Bearer sk-live-abcdef123456", "X-Trace: 7"],
            "env": ["GITHUB_TOKEN=ghp_abcdefghijklmnop1234", "HOME=/usr/local"]
        }]),
    );

    let out = crate::handlers::mcp::McpHandler::new()
        .handle_cmd("servers", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let s = &out["servers"][0];
    assert_eq!(
        s["headers"][0], "Authorization: Bear****3456",
        "敏感 header 值部必须脱敏"
    );
    assert_eq!(s["headers"][1], "X-Trace: 7", "非敏感 header 原样");
    assert_eq!(
        s["env"][0], "GITHUB_TOKEN=ghp_****1234",
        "敏感 env 值部必须脱敏"
    );
    assert_eq!(s["env"][1], "HOME=/usr/local", "非敏感 env 原样");
    assert!(
        !out.to_string().contains("sk-live-abcdef123456")
            && !out.to_string().contains("ghp_abcdefghijklmnop1234"),
        "响应任何位置不得出现原始凭据"
    );
}

#[tokio::test]
async fn mcp_server_add_rejects_masked_values() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    seed_mcp_config(&dir, serde_json::json!([]));

    let err = crate::handlers::mcp::McpHandler::new()
        .handle_cmd(
            "server.add",
            Some(serde_json::json!({
                "name": "s1",
                "env": ["API_KEY=sk-1****cdef"]
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("掩码值"), "{err}");
}

#[tokio::test]
async fn mcp_server_update_restores_masked_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "s1", "url": "http://x", "env": ["API_KEY=sk-real-secret-xyz"] }]),
    );
    let h = crate::handlers::mcp::McpHandler::new();

    // UI 把脱敏形态原样存回（连带改 url）→ env 掩码按键还原存量原值。
    h.handle_cmd(
        "server.update",
        Some(serde_json::json!({
            "name": "s1",
            "url": "http://new",
            "env": ["API_KEY=sk-re****-xyz"]
        })),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();

    let raw = std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap();
    assert!(
        raw.contains("sk-real-secret-xyz"),
        "原值必须还在盘上: {raw}"
    );
    assert!(!raw.contains("****"), "掩码绝不能落盘: {raw}");

    // 找不到同键原值的掩码条目 → loud 拒绝。
    let err = h
        .handle_cmd(
            "server.update",
            Some(serde_json::json!({ "name": "s1", "env": ["OTHER_KEY=****"] })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("重新输入完整值"), "{err}");
}

#[tokio::test]
async fn mcp_config_save_restores_masked_entries_wholesale() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "s1", "env": ["API_KEY=sk-keep-original-000"] }]),
    );

    // 整包回存（config.get 脱敏形态 → UI 改别的字段 → 整包存回）。
    crate::handlers::mcp::McpHandler::new()
        .handle_cmd(
            "config.save",
            Some(serde_json::json!({
                "enabled": true,
                "servers": [{ "name": "s1", "env": ["API_KEY=sk-ke****-000"] }]
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

    let raw = std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap();
    assert!(raw.contains("sk-keep-original-000"), "原值必须保留: {raw}");
    assert!(!raw.contains("****"), "掩码绝不能落盘: {raw}");
}

// -- ② + ③ cluster 面（feature=cluster） --------------------------------------

#[cfg(feature = "cluster")]
mod cluster_mask_tests {
    use super::*;

    fn seed_cluster_config(dir: &tempfile::TempDir, json: serde_json::Value) {
        let cfg_dir = dir.path().join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("config.cluster.json"), json.to_string()).unwrap();
    }

    #[tokio::test]
    async fn cluster_config_get_masks_token() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        seed_cluster_config(
            &dir,
            serde_json::json!({ "enabled": true, "port": 47100, "token": "udp-discovery-secret-123" }),
        );

        let out = crate::handlers::cluster::ClusterHandler::new()
            .handle_cmd("config.get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out["token"], "udp-****-123", "token 必须脱敏回显");
        assert_eq!(out["port"], 47100, "非敏感字段原样");
        assert!(
            !out.to_string().contains("udp-discovery-secret-123"),
            "响应任何位置不得出现原始 token"
        );
    }

    #[tokio::test]
    async fn cluster_config_save_restores_masked_token() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        seed_cluster_config(
            &dir,
            serde_json::json!({ "enabled": true, "token": "udp-real-token-value" }),
        );
        let h = crate::handlers::cluster::ClusterHandler::new();

        // UI 回存脱敏形态（连带改 port）→ token 还原存量原值。
        h.handle_cmd(
            "config.save",
            Some(serde_json::json!({ "enabled": true, "port": 47200, "token": "udp****alue" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

        let raw = std::fs::read_to_string(dir.path().join("config/config.cluster.json")).unwrap();
        assert!(raw.contains("udp-real-token-value"), "原值必须保留: {raw}");
        assert!(!raw.contains("****"), "掩码绝不能落盘: {raw}");

        // 无存量文件时回存掩码 → loud 拒绝。
        let dir2 = tempfile::tempdir().unwrap();
        let ctx2 = make_ctx(&dir2);
        let err = crate::handlers::cluster::ClusterHandler::new()
            .handle_cmd(
                "config.save",
                Some(serde_json::json!({ "token": "****" })),
                &ctx2,
            )
            .await
            .unwrap_err();
        assert!(err.contains("重新输入完整值"), "{err}");
    }
}

// -- ③ channels 面（存量脱敏回显的回程还原） -----------------------------------

mod channels_mask_tests {
    use super::*;

    fn seed_channels_config(dir: &tempfile::TempDir, json: serde_json::Value) {
        // 主 config.json 在 home 根（channels.rs load_config 的 CLI 回落路径）。
        std::fs::write(dir.path().join("config.json"), json.to_string()).unwrap();
    }

    #[tokio::test]
    async fn channels_update_restores_masked_fields() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        seed_channels_config(
            &dir,
            serde_json::json!({
                "channels": {
                    "slack": { "enabled": true, "bot_token": "xoxb-real-secret-token" }
                }
            }),
        );
        let h = crate::handlers::channels::ChannelsHandler::new();

        // get 回显是脱敏形态；UI 原样存回（连带改 enabled）→ 还原原值。
        let got = h
            .handle_cmd("get", Some(serde_json::json!({ "name": "slack" })), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got["config"]["bot_token"], "xoxb****oken", "回显必须脱敏");

        h.handle_cmd(
            "update",
            Some(serde_json::json!({
                "name": "slack",
                "config": { "enabled": false, "bot_token": "xoxb****oken" }
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

        let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(
            raw.contains("xoxb-real-secret-token"),
            "原值必须保留: {raw}"
        );
        assert!(!raw.contains("****"), "掩码绝不能落盘: {raw}");
    }
}

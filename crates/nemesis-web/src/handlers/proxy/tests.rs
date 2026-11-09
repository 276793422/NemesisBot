//! proxy.keys.* handler 测试（2026-11-03 LLM代理调用方key管理）。
//!
//! 每用例独立 tempdir（文件互不共享），无进程级单例可污染——不持锁。
//! keys.add 的防呆与查重在 config 层（`proxy_keys_file` 单测已覆盖底层），
//! 本文件钉 handler 层投影与透传：参数解析 / 主 token 防呆 / 热生效联动。

use super::ProxyHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::proxy_keys::match_proxy_caller;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir, auth_token: &str) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        auth_token: auth_token.to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws),
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
        workspace: Some(dir.path().to_string_lossy().to_string()),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

async fn keys_list(ctx: &RequestContext) -> serde_json::Value {
    ProxyHandler
        .handle_cmd("keys.list", None, ctx)
        .await
        .expect("keys.list ok")
        .expect("keys.list returns payload")
}

#[tokio::test]
async fn keys_list_empty_workspace_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    let out = keys_list(&ctx).await;
    assert_eq!(out["file_exists"], serde_json::json!(false));
    assert_eq!(out["keys"], serde_json::json!([]));
    assert!(
        out["path"]
            .as_str()
            .unwrap()
            .ends_with("config.proxy_keys.json")
    );
}

#[tokio::test]
async fn keys_add_auto_generates_nbk_key() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    let out = ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "note": "第一台"})),
            &ctx,
        )
        .await
        .expect("add ok")
        .expect("payload");
    let key = out["key"]["key"].as_str().expect("key string");
    assert!(key.starts_with("nbk_"), "auto key shape: {key}");
    assert_eq!(key.len(), 4 + 32, "nbk_ + 32 hex, got: {key}");
    assert_eq!(out["key"]["name"], "bot-a");
    assert_eq!(out["key"]["enabled"], serde_json::json!(true));
    assert_eq!(out["key"]["note"], "第一台");
    assert!(out["key"]["created_at"].as_i64().unwrap_or(0) > 0, "补戳");
    // 落盘可回读（list 面一致）。
    let list = keys_list(&ctx).await;
    assert_eq!(list["file_exists"], serde_json::json!(true));
    assert_eq!(list["keys"].as_array().unwrap().len(), 1);
    assert_eq!(list["keys"][0]["key"], serde_json::json!(key));
}

#[tokio::test]
async fn keys_add_explicit_key_and_trims_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    let out = ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "  bot-b  ", "key": "  nbk_bbb  "})),
            &ctx,
        )
        .await
        .expect("add ok")
        .expect("payload");
    assert_eq!(out["key"]["name"], "bot-b", "name trim");
    assert_eq!(out["key"]["key"], "nbk_bbb", "key trim");
}

#[tokio::test]
async fn keys_add_empty_name_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    let err = ProxyHandler
        .handle_cmd("keys.add", Some(serde_json::json!({"name": "   "})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("名称"), "got: {err}");
}

#[tokio::test]
async fn keys_add_duplicate_name_and_key_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "key": "nbk_aaa"})),
            &ctx,
        )
        .await
        .expect("first add ok");
    let err = ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "key": "nbk_other"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("名称已存在"), "got: {err}");
    let err = ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-c", "key": "nbk_aaa"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("key 值已存在"), "got: {err}");
}

#[tokio::test]
async fn keys_add_rejects_master_token_as_key() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    let err = ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "key": "master-tok"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("主 token"), "got: {err}");
    // 空主 token（免闸部署）不误伤：非空 key 恒 ≠ ""。
    let dir2 = tempfile::tempdir().unwrap();
    let ctx2 = make_ctx(&dir2, "");
    ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "key": "nbk_aaa"})),
            &ctx2,
        )
        .await
        .expect("empty master must not reject");
}

#[tokio::test]
async fn keys_add_makes_caller_hot_matchable() {
    // handler 写盘 → 鉴权回落读点（mtime 缓存）即刻命中 = 热生效联动。
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    let out = ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "key": "nbk_hot"})),
            &ctx,
        )
        .await
        .expect("add ok")
        .expect("payload");
    let state_arc = ctx.state.clone();
    let caller = match_proxy_caller(&state_arc, "nbk_hot", "/v1/chat/completions");
    assert!(caller.is_some(), "新增 key 必须即刻命中鉴权回落");
    assert_eq!(caller.unwrap().0.as_str(), "bot-a");
    assert!(
        out["note"].as_str().unwrap().contains("即刻生效"),
        "生效语义提示: {}",
        out["note"]
    );
}

#[tokio::test]
async fn keys_remove_removes_and_reports_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    ProxyHandler
        .handle_cmd("keys.add", Some(serde_json::json!({"name": "bot-a"})), &ctx)
        .await
        .expect("add ok");
    let out = ProxyHandler
        .handle_cmd(
            "keys.remove",
            Some(serde_json::json!({"name": "bot-a"})),
            &ctx,
        )
        .await
        .expect("remove ok")
        .expect("payload");
    assert_eq!(out["removed"], "bot-a");
    let list = keys_list(&ctx).await;
    assert_eq!(list["keys"].as_array().unwrap().len(), 0);
    let err = ProxyHandler
        .handle_cmd(
            "keys.remove",
            Some(serde_json::json!({"name": "ghost"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "got: {err}");
}

#[tokio::test]
async fn keys_set_enabled_roundtrip_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    ProxyHandler
        .handle_cmd(
            "keys.add",
            Some(serde_json::json!({"name": "bot-a", "key": "nbk_aaa"})),
            &ctx,
        )
        .await
        .expect("add ok");
    let out = ProxyHandler
        .handle_cmd(
            "keys.set_enabled",
            Some(serde_json::json!({"name": "bot-a", "enabled": false})),
            &ctx,
        )
        .await
        .expect("set ok")
        .expect("payload");
    assert_eq!(out["key"]["enabled"], serde_json::json!(false));
    let list = keys_list(&ctx).await;
    assert_eq!(list["keys"][0]["enabled"], serde_json::json!(false));
    // 幂等重设同值 = 成功不报错。
    ProxyHandler
        .handle_cmd(
            "keys.set_enabled",
            Some(serde_json::json!({"name": "bot-a", "enabled": false})),
            &ctx,
        )
        .await
        .expect("idempotent set ok");
    let err = ProxyHandler
        .handle_cmd(
            "keys.set_enabled",
            Some(serde_json::json!({"name": "ghost", "enabled": true})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "got: {err}");
}

#[tokio::test]
async fn keys_commands_missing_data_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, "master-tok");
    for cmd in ["keys.add", "keys.remove", "keys.set_enabled"] {
        let err = ProxyHandler.handle_cmd(cmd, None, &ctx).await.unwrap_err();
        assert!(err.contains("缺少"), "{cmd}: got: {err}");
    }
}

#[tokio::test]
async fn commands_list_includes_keys_family() {
    let cmds = ProxyHandler.commands();
    for c in [
        "get",
        "save",
        "keys.list",
        "keys.add",
        "keys.remove",
        "keys.set_enabled",
    ] {
        assert!(cmds.contains(&c), "commands() 缺 {c}");
    }
}

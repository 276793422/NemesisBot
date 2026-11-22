//! (quality-hardening goal 冲刺, web): ChannelsHandler error arms
//! and sensitive-field masking the gated `mod tests` skips — missing
//! config.json load failure, update without a config payload, update whose
//! payload fails channels re-parse, and `mask_sensitive_fields` recursion.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_state() -> Arc<AppState> {
    Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
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
        mail_plugin: None,
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
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
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
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: None,
        home: Some(dir.path().to_string_lossy().to_string()),
        state: make_state(),
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn channels_list_with_malformed_config_file_errors() {
    // Missing config.json falls back to defaults (Ok) — a broken file trips
    // the load error arm.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{definitely not json").unwrap();
    let ctx = make_ctx(&dir, false);
    let handler = ChannelsHandler::new();
    let err = handler.handle_cmd("list", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to load config"), "{err}");
}

#[tokio::test]
async fn channels_update_missing_config_field_and_bad_reparse_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ChannelsHandler::new();

    // "web" exists in default channels config, but the payload has no
    // "config" sub-object.
    let err = handler
        .handle_cmd("update", Some(serde_json::json!({ "name": "web" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing config field");

    // `enabled` must stay a bool — a string breaks the channels re-parse.
    let err = handler
        .handle_cmd(
            "update",
            Some(serde_json::json!({
                "name": "web",
                "config": { "enabled": "yes" }
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to parse updated channels"), "{err}");
}

/// BUG-20261120-027 回归锁：单通道 `config: []` 曾在外层 object 的字段级
/// visit_seq 臂变成「全默认」合法值——该通道配置被静默重置还回
/// updated:true。形状闸在字段赋值前拦下。
#[tokio::test]
async fn channels_update_array_config_loud_rejected_and_disk_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ChannelsHandler::new();

    let err = handler
        .handle_cmd(
            "update",
            Some(serde_json::json!({ "name": "web", "config": [] })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("must be a JSON object"), "{err}");
    assert!(err.contains("array"), "{err}");

    // 盘上 web 通道配置原样（仍是 object，未被重置）。
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert!(
        on_disk["channels"]["web"].is_object(),
        "web channel config must remain an object on disk"
    );
}

#[test]
fn mask_sensitive_fields_masks_sensitive_and_recurses() {
    let v = serde_json::json!({
        "name": "web",
        "token": "short",
        "api_key": "sk-1234567890abcdef",
        "empty_token": "",
        "nested": {
            "password": "hunter2password-long",
            "plain": "visible"
        },
        "list": [{ "secret": "0123456789abc" }],
    });
    let out = mask_sensitive_fields(v);
    assert_eq!(out["name"], "web");
    assert_eq!(out["token"], "****", "short values fully masked");
    assert_eq!(out["api_key"], "sk-1****cdef");
    assert_eq!(out["empty_token"], "", "empty sensitive values left empty");
    assert_eq!(out["nested"]["password"], "hunt****long");
    assert_eq!(out["nested"]["plain"], "visible");
    assert_eq!(out["list"][0]["secret"], "0123****9abc");
}

// BUG-20261122-043 回归：复合命名凭据字段的脱敏名单缺口。is_sensitive_field
// 是全等匹配语义（contains 会误伤 tokens_used 等统计字段），此前漏
// verification_token（feishu）/ channel_secret + channel_access_token（line）/
// encoding_aes_key（wecom）/ private_key（nostr 64-hex 私钥）/
// imap_password + smtp_password（email 明文密码）七个真实存在的 typed
// config 字段——channels.config 输出把它们的原文回传前端。
#[test]
fn composite_named_credential_fields_are_masked() {
    let v = serde_json::json!({
        "feishu": { "verification_token": "feishu-verify-token-long" },
        "line": {
            "channel_secret": "line-channel-secret-1234",
            "channel_access_token": "line-channel-access-token-long"
        },
        "wecom": { "encoding_aes_key": "wecom-aes-key-43-chars-long-here" },
        "nostr": { "private_key": "00aa11bb22cc33dd44ee55ff66aa77bb88cc99dd00ee11ff22aa33bb44cc55dd" },
        "email": { "imap_password": "imap-passphrase-123", "smtp_password": "smtp-passphrase-456" },
        // 路径字段刻意不 mask（路径非凭据值，mask 破坏 UI 显示文件位置）。
        "mtls": { "client_key_path": "C:/certs/client.key" },
        // 统计字段不因 contains 语义误伤（全等匹配守卫）。
        "usage": { "tokens_used": 12345 }
    });
    let out = mask_sensitive_fields(v);
    assert_eq!(out["feishu"]["verification_token"], "feis****long");
    assert_eq!(out["line"]["channel_secret"], "line****1234");
    assert_eq!(out["line"]["channel_access_token"], "line****long");
    assert_eq!(out["wecom"]["encoding_aes_key"], "weco****here");
    assert_eq!(out["nostr"]["private_key"], "00aa****55dd");
    assert_eq!(out["email"]["imap_password"], "imap****-123");
    assert_eq!(out["email"]["smtp_password"], "smtp****-456");
    assert_eq!(
        out["mtls"]["client_key_path"], "C:/certs/client.key",
        "路径字段不 mask"
    );
    assert_eq!(out["usage"]["tokens_used"], 12345, "统计字段不误伤");
}

// mask/restore 对称闭环（restore_masked_named_fields 与 mask 共用
// is_sensitive_field 名单）：「查看（掩码）→ 保存」循环必须把新名单字段的
// 掩码还原成原值，而不是把 `****` 字符串持久化成真值（比泄漏更糟的静默
// 配置破坏面）。
#[test]
fn composite_named_fields_restore_masked_values_roundtrip() {
    let existing = serde_json::json!({
        "feishu": { "verification_token": "feishu-verify-token-long" },
        "nostr": { "private_key": "00aa11bb22cc33dd44ee55ff66aa77bb88cc99dd00ee11ff22aa33bb44cc55dd" },
        "email": { "smtp_password": "smtp-passphrase-456" }
    });
    let mut incoming = serde_json::json!({
        "feishu": { "verification_token": "feis****-long" },
        "nostr": { "private_key": "00aa****55dd" },
        "email": { "smtp_password": "smtp****-456" }
    });
    crate::handlers::restore_masked_named_fields(&mut incoming, &existing)
        .expect("masked values must restore from existing");
    assert_eq!(
        incoming["feishu"]["verification_token"],
        "feishu-verify-token-long"
    );
    assert_eq!(
        incoming["nostr"]["private_key"],
        "00aa11bb22cc33dd44ee55ff66aa77bb88cc99dd00ee11ff22aa33bb44cc55dd"
    );
    assert_eq!(incoming["email"]["smtp_password"], "smtp-passphrase-456");
}

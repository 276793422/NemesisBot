//! events handler 单测（`events` feature 门控）：
//! - status：槽未装配诚实报错 / 快照在位返回序列化面；
//! - rules.get：文件缺失 = 空规则集（首次启动正常态）；
//! - rules.save：合法规则落盘回读 / schema 非法 loud 拒 / 校验警告随响应。

use super::EventsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// Same AppState scaffold as automations tests（agent_loop None etc.），
/// 外加 events_snapshot 槽位（调用方决定 None 或真句柄）。
fn make_ctx_with_snapshot(
    dir: &tempfile::TempDir,
    snap: Option<std::sync::Arc<parking_lot::RwLock<nemesis_events::EventSnapshot>>>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = std::sync::Arc::new(AppState {
        cors_manager: None,
        events_snapshot: snap,
        auth_token: String::new(),
        session_count: std::sync::Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: std::sync::Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
        event_hub: std::sync::Arc::new(EventHub::new()),
        running: std::sync::Arc::new(AtomicBool::new(true)),
        session_manager: std::sync::Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
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
        webhook_rate_limiter: std::sync::Arc::new(
            crate::handlers::workflow::WebhookRateLimiter::new(),
        ),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
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
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    make_ctx_with_snapshot(dir, None)
}

fn snap_handle(
    s: nemesis_events::EventSnapshot,
) -> std::sync::Arc<parking_lot::RwLock<nemesis_events::EventSnapshot>> {
    std::sync::Arc::new(parking_lot::RwLock::new(s))
}

/// 合法规则 JSON（preset session_key + fs watch）。
fn valid_rule_json(session: &str) -> serde_json::Value {
    serde_json::json!([{
        "id": "t-rule",
        "watch": {"kind": "fs", "path": "C:/tmp/watch", "glob": "*.log"},
        "deliver": {
            "session_key": format!("agent:main:session:{session}"),
            "prompt": "构建日志更新：{paths}",
            "cooldown_secs": 30
        }
    }])
}

#[tokio::test]
async fn status_unwired_is_honest() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = EventsHandler
        .handle_cmd("status", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("not available"),
        "槽未装配必须诚实报错，got: {err}"
    );
}

#[tokio::test]
async fn status_with_snapshot_serializes_stats_and_rules() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = nemesis_events::EventSnapshot {
        stats: nemesis_events::ManagerStats {
            delivered_events: 7,
            delivered_messages: 5,
            dropped_cooldown: 2,
            ..Default::default()
        },
        rules: vec![nemesis_events::RuleView {
            id: "r1".to_string(),
            kind: "fs".to_string(),
            path: "C:/tmp".to_string(),
            glob: Some("*.log".to_string()),
            session_key: "agent:main:session:x".to_string(),
            enabled: true,
            cooldown_secs: 30,
            merge_window_secs: 5,
            max_per_hour: 10,
            detail: String::new(),
        }],
        pending: vec![nemesis_events::PendingView {
            rule_id: "r1".to_string(),
            count: 3,
        }],
        sys_current: Some(nemesis_events::SysCurrentView {
            cpu: 23.5,
            mem_percent: 61.0,
            disks: vec![nemesis_events::SysDiskView {
                mount: "C:\\".to_string(),
                used_percent: 88.0,
            }],
        }),
    };
    let ctx = make_ctx_with_snapshot(&dir, Some(snap_handle(snapshot)));
    let out = EventsHandler
        .handle_cmd("status", None, &ctx)
        .await
        .unwrap()
        .expect("status returns payload");
    assert_eq!(out["stats"]["delivered_events"], 7);
    assert_eq!(out["stats"]["delivered_messages"], 5);
    assert_eq!(out["stats"]["dropped_cooldown"], 2);
    assert_eq!(out["rules"][0]["id"], "r1");
    assert_eq!(out["rules"][0]["cooldown_secs"], 30);
    assert_eq!(out["pending"][0]["count"], 3);
    // sys 当前值透传面（EventsView sys 当前行数据源）。
    assert_eq!(out["sys_current"]["cpu"], 23.5);
    assert_eq!(out["sys_current"]["mem_percent"], 61.0);
    assert_eq!(out["sys_current"]["disks"][0]["used_percent"], 88.0);
}

#[tokio::test]
async fn rules_get_missing_file_returns_empty_set() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let out = EventsHandler
        .handle_cmd("rules.get", None, &ctx)
        .await
        .unwrap()
        .expect("missing file = 空规则集非错误");
    assert_eq!(out["rules"].as_array().map(Vec::len), Some(0));
    assert_eq!(out["warnings"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn rules_save_roundtrip_then_get() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "rules": valid_rule_json("abc-123") });
    let out = EventsHandler
        .handle_cmd("rules.save", Some(data), &ctx)
        .await
        .unwrap()
        .expect("合法规则保存成功");
    assert_eq!(out["saved"], true);
    assert_eq!(out["warnings"].as_array().map(Vec::len), Some(0));

    // 回读：磁盘文件在 + rules.get 可见。
    let path = dir.path().join("config").join("events.json");
    assert!(path.exists(), "rules 文件应落盘 {}", path.display());
    let got = EventsHandler
        .handle_cmd("rules.get", None, &ctx)
        .await
        .unwrap()
        .expect("rules.get after save");
    assert_eq!(got["rules"][0]["id"], "t-rule");
    assert_eq!(
        got["rules"][0]["deliver"]["session_key"],
        "agent:main:session:abc-123"
    );
}

#[tokio::test]
async fn rules_save_schema_reject_is_loud() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // 缺 deliver.session_key（必填）→ serde 解析失败 → handler Err。
    let data = serde_json::json!({ "rules": [{
        "id": "bad",
        "watch": {"kind": "fs", "path": "C:/tmp"},
        "deliver": {"prompt": "x"}
    }]});
    let err = EventsHandler
        .handle_cmd("rules.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("schema invalid"),
        "缺必填字段须 loud 拒，got: {err}"
    );
}

#[tokio::test]
async fn rules_save_collects_validation_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // session_key 非 preset 形态 → serde 合法、校验剔除 + warning。
    let data = serde_json::json!({ "rules": [{
        "id": "t2",
        "watch": {"kind": "fs", "path": "C:/tmp"},
        "deliver": {"session_key": "bare-id", "prompt": "x"}
    }]});
    let out = EventsHandler
        .handle_cmd("rules.save", Some(data), &ctx)
        .await
        .unwrap()
        .expect("serde 合法即保存成功");
    assert_eq!(out["saved"], true);
    let warnings = out["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        !warnings.is_empty() && warnings[0].as_str().unwrap_or("").contains("preset"),
        "校验警告应随响应（非 preset 形态剔除），got: {warnings:?}"
    );
}

#[tokio::test]
async fn unknown_command_is_loud() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = EventsHandler
        .handle_cmd("nope", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown command"), "got: {err}");
}

/// commands() 清单钉死（L1 注册表面漂移闸）。
#[test]
fn commands_surface_is_pinned() {
    assert_eq!(
        EventsHandler.commands(),
        &["status", "rules.get", "rules.save"]
    );
    assert_eq!(EventsHandler.module_name(), "events");
}

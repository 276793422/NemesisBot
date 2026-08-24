//! Tests for the sandbox handler.
//!
//! Kept in a separate file (not inlined in `sandbox.rs`) per the project's
//! "tests live in `<stem>/tests.rs`" discipline.

use super::*;

/// Field-merge fix: editing `enabled`/`sandbox` via [`update_executor`] must NOT
/// reset `allow_network`. The pre-fix code did `c.executor = Some({enabled, sandbox})`,
/// which clobbered `allow_network` on every `start`/`stop`. This test exercises the
/// CLI / no-store fallback path (read-merge-write of config.json) — the path that
/// carried the bug — and guards against regressing back to an overwrite.
#[test]
fn update_executor_preserves_allow_network_across_sibling_edits() {
    // The CLI fallback only runs when no process-global ConfigStore is installed.
    // A test process normally has none; if something installed one we can't isolate
    // the CLI path, so skip rather than flake (mirrors the test-isolation stance).
    if nemesis_config::global().is_some() {
        eprintln!(
            "skip update_executor_preserves_allow_network: \
             process-global ConfigStore installed, can't isolate CLI path"
        );
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": true, "allow_network": true } }"#,
    )
    .expect("seed config.json");

    // Simulate `stop`: flip enabled+sandbox off — exactly what set_executor_config does.
    update_executor(home, |e| {
        e.enabled = false;
        e.sandbox = false;
    })
    .expect("update_executor");

    let executor = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(home.join("config.json")).expect("read config.json"),
    )
    .expect("parse config.json")
    .get("executor")
    .expect("executor section")
    .clone();
    assert_eq!(executor["enabled"], false);
    assert_eq!(executor["sandbox"], false);
    assert_eq!(
        executor["allow_network"],
        true,
        "allow_network must survive enabled/sandbox edits (field-merge fix regressed)"
    );
}

// ---------------------------------------------------------------------------
// P5: overview（平台自适应总览）+ set_config（逐字段开关变更）
// ---------------------------------------------------------------------------

/// Same AppState scaffold as hooks/models tests (agent_loop None etc.).
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

/// overview 结构断言（不依赖机器能力——Sandboxie/landlock 是否就绪随环境变，
/// 结构与 live 开关回读不变）。
#[tokio::test]
async fn overview_shape_and_live_executor_switches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": true, "allow_network": true, "strict": true } }"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);
    let h = SandboxHandler::new();
    let v = h
        .handle_cmd("overview", None, &ctx)
        .await
        .expect("overview ok")
        .expect("overview returns a payload");

    assert!(
        ["windows", "linux", "macos", "other"].contains(&v["platform"].as_str().unwrap_or("")),
        "platform: {}",
        v["platform"]
    );
    // 四个开关 live 回读（与种子 config 一致）
    assert_eq!(v["executor"]["enabled"], true);
    assert_eq!(v["executor"]["sandbox"], true);
    assert_eq!(v["executor"]["allow_network"], true);
    assert_eq!(v["executor"]["strict"], true, "strict must be surfaced (P5-2)");
    // 后端探测：结构随平台，但一定是对象且带 kind
    let kind = v["backend_probe"]["kind"].as_str().unwrap_or("");
    if cfg!(target_os = "windows") {
        assert_eq!(kind, "sandboxie");
        assert!(v["backend_probe"]["start_exe_present"].is_boolean());
        assert!(v["backend_probe"]["sbiesvc_running"].is_boolean());
        assert!(v["backend_probe"]["engine_owned"].is_boolean());
    } else {
        assert_eq!(kind, "userland");
        assert!(v["backend_probe"]["backends"].is_array());
        // selected 为 null（无后端）或字符串（后端名）
        let sel = &v["backend_probe"]["selected"];
        assert!(sel.is_null() || sel.is_string(), "selected: {sel}");
    }
    assert!(v["ready"].is_boolean(), "ready must be a bool");
}

/// set_config 逐字段合并：改 strict 不动其余三个；改 enabled/sandbox 不动
/// strict / allow_network。走 CLI/no-store 回退路径（与上方 field-merge 测试
/// 同款 skip 策略）。
#[tokio::test]
async fn set_config_merges_field_by_field() {
    if nemesis_config::global().is_some() {
        eprintln!(
            "skip set_config_merges_field_by_field: \
             process-global ConfigStore installed, can't isolate CLI path"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let seed = r#"{ "executor": { "enabled": true, "sandbox": false, "allow_network": true, "strict": false } }"#;
    std::fs::write(dir.path().join("config.json"), seed).unwrap();
    let ctx = make_ctx(&dir);
    let h = SandboxHandler::new();

    // 1) 只改 strict → 其余兄弟字段保留
    let resp = h
        .handle_cmd("set_config", Some(serde_json::json!({ "strict": true })), &ctx)
        .await
        .expect("set_config ok")
        .expect("set_config returns a payload");
    assert_eq!(resp["executor"]["strict"], true);
    assert_eq!(resp["executor"]["enabled"], true, "enabled must survive");
    assert_eq!(resp["executor"]["sandbox"], false, "sandbox must survive");
    assert_eq!(resp["executor"]["allow_network"], true, "allow_network must survive");

    // 2) 只改 enabled+sandbox → strict/allow_network 保留
    let resp = h
        .handle_cmd(
            "set_config",
            Some(serde_json::json!({ "enabled": false, "sandbox": true })),
            &ctx,
        )
        .await
        .expect("set_config ok")
        .expect("set_config returns a payload");
    assert_eq!(resp["executor"]["enabled"], false);
    assert_eq!(resp["executor"]["sandbox"], true);
    assert_eq!(resp["executor"]["strict"], true, "strict must survive (P5-2)");
    assert_eq!(resp["executor"]["allow_network"], true);

    // 3) 出现但非 bool → 明确报错（不静默忽略）
    let err = h
        .handle_cmd(
            "set_config",
            Some(serde_json::json!({ "strict": "yes" })),
            &ctx,
        )
        .await
        .expect_err("non-bool must be rejected");
    assert!(err.contains("strict"), "err names the bad field: {err}");

    // 4) 空载荷 → 报错
    h.handle_cmd("set_config", Some(serde_json::json!({})), &ctx)
        .await
        .expect_err("empty payload must be rejected");
}

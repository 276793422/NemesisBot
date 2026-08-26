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

// ---------------------------------------------------------------------------
// Phase 3 批次 18（2026-08-25）：parse_selection / current_executor 回退 /
// home_of / 只读探测命令（status/check/pending）/ set_network 缺参。
// 真 Sandboxie 交互（install/start/stop/open_box 提权子进程）→ 结构性豁免。
// ---------------------------------------------------------------------------

#[test]
fn parse_selection_variants() {
    // 空 / 缺字段 → (false, [])
    let (all, files) = parse_selection(&serde_json::json!({}));
    assert!(!all);
    assert!(files.is_empty());
    // 显式 false
    let (all, files) = parse_selection(&serde_json::json!({ "all": false }));
    assert!(!all);
    assert!(files.is_empty());
    // all=true
    let (all, files) = parse_selection(&serde_json::json!({ "all": true }));
    assert!(all);
    assert!(files.is_empty());
    // files 列表（非字符串项被过滤）
    let (all, files) = parse_selection(&serde_json::json!({
        "all": false,
        "files": ["abc.txt", 42, "def.rs", null]
    }));
    assert!(!all);
    assert_eq!(files, vec!["abc.txt".to_string(), "def.rs".to_string()]);
}

#[test]
fn current_executor_fallback_variants() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // 1) 无 config.json → 全默认（false）
    let e = current_executor(home);
    assert!(!e.enabled && !e.sandbox && !e.allow_network && !e.strict);
    // 2) 损坏 JSON → 全默认
    std::fs::write(home.join("config.json"), "<<<").unwrap();
    let e = current_executor(home);
    assert!(!e.enabled && !e.allow_network);
    // 3) 有 executor 段 → 正确解析
    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true, "allow_network": true, "strict": true } }"#,
    )
    .unwrap();
    let e = current_executor(home);
    assert!(e.enabled);
    assert!(e.allow_network);
    assert!(e.strict);
    assert!(!e.sandbox, "未出现的字段保持默认");
    // current_allow_network 走同一解析
    assert!(current_allow_network(home));
}

#[tokio::test]
async fn home_of_missing_home_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx(&dir);
    ctx.home = None;
    let err = home_of(&ctx).unwrap_err();
    assert_eq!(err, "home not configured");
    // 命令层同样拒绝（status 是只读命令，最先触达 home_of）
    let h = SandboxHandler::new();
    let err = h.handle_cmd("status", None, &ctx).await.unwrap_err();
    assert_eq!(err, "home not configured");
}

#[tokio::test]
async fn readonly_probe_commands_on_empty_home() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SandboxHandler::new();

    // status：tempdir 里没有 Start.exe → ready 恒 false（sbiesvc/sbiedrv 随环境变，不断言）
    let v = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(v["ready"], false);
    assert_eq!(v["start_exe_present"], false);
    assert_eq!(v["allow_network"], false, "无 config.json → 默认禁网");
    assert!(v["box_root"].is_string());

    // check：Sandboxie 文件未获取；7z 探测会扫系统安装（本机装了 7-Zip 时
    // available=true source=system），只断言结构不断言环境态
    let v = h.handle_cmd("check", None, &ctx).await.unwrap().unwrap();
    assert!(v["seven_zip"]["available"].is_boolean());
    assert!(v["seven_zip"]["source"].is_string());
    assert_eq!(v["sandboxie"]["files_acquired"], false);
    assert_eq!(v["allow_network"], false);

    // pending：box 目录不存在 → 空列表（enumerate_box 对缺失目录返回 Ok(vec![])）
    let v = h.handle_cmd("pending", None, &ctx).await.unwrap().unwrap();
    assert_eq!(v["files"], serde_json::json!([]));

    // commit / delete 空选择 → 0 total 0
    let v = h
        .handle_cmd("commit", Some(serde_json::json!({ "all": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["committed"], 0);
    assert_eq!(v["total"], 0);
    let v = h
        .handle_cmd("delete", Some(serde_json::json!({ "all": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["deleted"], 0);
    assert_eq!(v["total"], 0);

    // open_explorer：引擎未就绪 → 明确报错（不 spawn）
    let err = h.handle_cmd("open_explorer", None, &ctx).await.unwrap_err();
    assert!(err.contains("sandbox not ready"), "err: {err}");
}

#[tokio::test]
async fn set_network_requires_enabled_bool() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SandboxHandler::new();
    // 缺 enabled / 非 bool → 明确报错
    let err = h.handle_cmd("set_network", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires"), "err: {err}");
    let err = h
        .handle_cmd("set_network", Some(serde_json::json!({ "enabled": "yes" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("requires"), "err: {err}");
}

/// Windows 上 set_network 完整路径：config + Sandboxie.ini 都落盘；
/// Start.exe /reload 因路径不存在而失败（tempdir home 无引擎），但
/// 前两步的副作用已发生——这正是「reload 失败不回滚 ini」的现状。
#[cfg(target_os = "windows")]
#[tokio::test]
async fn set_network_writes_config_and_ini_despite_reload_failure() {
    let dir = tempfile::tempdir().unwrap();
    // update_executor 的 CLI 回退是 read-merge-write：先种一个空 config
    std::fs::write(dir.path().join("config.json"), "{}").unwrap();
    let ctx = make_ctx(&dir);
    let h = SandboxHandler::new();
    let r = h
        .handle_cmd("set_network", Some(serde_json::json!({ "enabled": true })), &ctx)
        .await;
    // tempdir home 无 Start.exe → spawn 失败 → 命令返回 Err
    assert!(r.is_err(), "reload spawn must fail without Start.exe: {r:?}");
    // config.json 的 allow_network 已写入
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cfg["executor"]["allow_network"], true);
    // Sandboxie.ini 已重写且网络开关 = y
    let home = dir.path().to_path_buf();
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    let ini = std::fs::read_to_string(&paths.ini_path).unwrap();
    assert!(ini.contains("AllowNetworkAccess=y"), "ini must enable network");
}

#[tokio::test]
async fn unknown_sandbox_command_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SandboxHandler::new();
    let err = h.handle_cmd("bogus", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown sandbox command"), "err: {err}");
}

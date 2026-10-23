//! `ziwei.*` WSAPI 用例（T0.5）：门关拒绝面 / list·show 只读面 / remove /
//! 未装配诚实报 / L1 `commands()` 清单。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 最小 RequestContext（vm_ws_tests make_ctx 同款；tempdir 同时当
/// workspace 与 home——config.json 由用例按需写入）。
fn make_ctx() -> (RequestContext, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
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
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    (
        RequestContext {
            session_id: "s".to_string(),
            chat_id: "c".to_string(),
            workspace: Some(ws),
            home: Some(dir.path().to_string_lossy().to_string()),
            state,
            auth_method: crate::session::AuthMethod::default(),
        },
        dir,
    )
}

fn write_config(dir: &tempfile::TempDir, ziwei_enabled: bool) {
    let cfg = serde_json::json!({ "ziwei": { "enabled": ziwei_enabled } });
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string(&cfg).unwrap(),
    )
    .unwrap();
}

async fn call(
    cmd: &str,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    ZiweiHandler.handle_cmd(cmd, data, ctx).await
}

/// 门关（默认 enabled=false）：全部命令诚实拒绝（T0.5 验收面）。
#[tokio::test]
async fn gate_closed_rejects_everything() {
    let (ctx, dir) = make_ctx();
    write_config(&dir, false);
    for cmd in ZiweiHandler.commands() {
        let err = call(cmd, &ctx, None).await.unwrap_err();
        assert_eq!(err, GATE_CLOSED_MSG, "命令 {cmd} 应被门关拒绝");
    }
}

/// 连 config.json 都没有（老用户）= 门关，同样拒绝且文案一致。
#[tokio::test]
async fn missing_config_treated_as_closed() {
    let (ctx, _dir) = make_ctx();
    let err = call("list", &ctx, None).await.unwrap_err();
    assert_eq!(err, GATE_CLOSED_MSG);
}

/// 门开：list 只读面（空库 → 空数组）。
#[tokio::test]
async fn gate_open_list_empty() {
    let (ctx, dir) = make_ctx();
    write_config(&dir, true);
    let out = call("list", &ctx, None).await.unwrap().unwrap();
    assert_eq!(out["profiles"].as_array().unwrap().len(), 0);
}

/// 门开：show/remove 走真数据层（入库 → show 三件套 → remove → show NotFound）。
#[tokio::test]
async fn gate_open_show_and_remove() {
    let (ctx, dir) = make_ctx();
    write_config(&dir, true);
    let store = ProfileStore::new(dir.path());
    let p = sample_profile_for_test();
    store.save(&p, None).unwrap();

    let out = call(
        "show",
        &ctx,
        Some(serde_json::json!({ "profile_id": p.profile_id })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["profile"]["profile_id"], p.profile_id.as_str());
    assert!(out["notes"].as_str().unwrap().is_empty());

    // 缺参诚实拒。
    assert!(call("show", &ctx, None).await.is_err());

    let out = call(
        "remove",
        &ctx,
        Some(serde_json::json!({ "profile_id": p.profile_id })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["removed"], p.profile_id.as_str());
    assert!(
        call(
            "show",
            &ctx,
            Some(serde_json::json!({ "profile_id": p.profile_id })),
        )
        .await
        .is_err()
    );
}

/// 门开：运行期/配置期命令未装配 → 诚实报（点出里程碑）。
/// M1 起 attach/detach/status 已实装；M2 起 invoke/refresh 已实装
/// （invoke 缺会话管理器=装配面诚实报；refresh 缺钩子=钩子未装配诚实报），
/// 未装配面收窄到 setup.*（M3）。
#[tokio::test]
async fn unwired_commands_honest() {
    let (ctx, dir) = make_ctx();
    write_config(&dir, true);
    // invoke 已实装：无全局会话管理器 → 装配面诚实报（非「未实装」）。
    let err = call(
        "invoke",
        &ctx,
        Some(serde_json::json!({ "profile_id": "p", "tool": "t" })),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("未装配") || err.contains("会话"),
        "invoke: {err}"
    );
    // refresh 已实装：钩子未安装（测试进程无 gateway）→ 钩子面诚实报。
    let err = call("refresh", &ctx, None).await.unwrap_err();
    assert!(err.contains("钩子未装配"), "refresh: {err}");
    // attach/detach 已实装：缺参走参数校验诚实拒（不再报未装配）
    for cmd in ["attach", "detach"] {
        let err = call(cmd, &ctx, None).await.unwrap_err();
        assert!(!err.contains("未装配"), "{cmd}: {err}");
    }
    // setup.* 已实装（M3）：缺参/无向导走参数与状态校验诚实拒（不再报未装配）。
    // select_target 在 windows 测试进程可能真去选 pid=1 → 期待任意诚实错误。
    for cmd in [
        "setup.select_target",
        "setup.snapshot",
        "setup.analyze",
        "setup.diff_window",
        "setup.candidates",
        "setup.annotate",
        "setup.define_struct",
        "setup.commit",
    ] {
        let err = call(cmd, &ctx, None).await.unwrap_err();
        assert!(!err.contains("unknown command"), "{cmd}: {err}");
        assert!(!err.contains("未装配"), "{cmd}: {err}");
    }
    // 无向导时，文件面步骤诚实报"无活跃配置向导"（annotate 缺 data 先报缺参）。
    let err = call("setup.commit", &ctx, None).await.unwrap_err();
    assert!(
        err.contains("无活跃配置向导") || err.contains("未收尾"),
        "commit: {err}"
    );
}

/// 门开：status 返回 enabled + 引擎探测面。
#[tokio::test]
async fn gate_open_status() {
    let (ctx, dir) = make_ctx();
    write_config(&dir, true);
    let out = call("status", &ctx, None).await.unwrap().unwrap();
    assert_eq!(out["enabled"], true);
    assert_eq!(out["setup_engine"]["disasm_archs"][0], "x64");
}

/// L1：命令清单与注册表口径一致（drift 纠偏）。
#[test]
fn l1_command_list_complete() {
    let expected = [
        "list",
        "show",
        "attach",
        "detach",
        "invoke",
        "resolve",
        "status",
        "remove",
        "refresh",
        "verify",
        "recover",
        "recover_confirm",
        "setup.select_target",
        "setup.snapshot",
        "setup.analyze",
        "setup.diff_window",
        "setup.candidates",
        "setup.annotate",
        "setup.define_struct",
        "setup.commit",
    ];
    assert_eq!(ZiweiHandler.commands(), expected);
}

fn sample_profile_for_test() -> nemesis_ziwei::profile::Profile {
    let mut vr = std::collections::BTreeMap::new();
    vr.insert("ProductName".to_string(), "DataMon".to_string());
    nemesis_ziwei::profile::Profile {
        schema_version: 1,
        profile_id: "zw-pro-test0001".to_string(),
        name: "测试靶标-导出".to_string(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: nemesis_ziwei::profile::ProfileStatus::Active,
        target: nemesis_ziwei::profile::TargetFingerprint {
            exe_name: "DataMon.exe".to_string(),
            exe_path_hint: None,
            machine: nemesis_ziwei::profile::Machine::X64,
            file_sha256: None,
            version_resource: vr,
            verify_policy: nemesis_ziwei::profile::VerifyPolicy::HashOrVersion,
        },
        attach: Default::default(),
        functions: vec![],
        data_structures: vec![],
        tools: vec![],
        verification: nemesis_ziwei::profile::Verification {
            on_attach: vec!["file_sha256".to_string()],
            stale_action: None,
        },
    }
}

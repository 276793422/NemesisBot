//! sandbox.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 tests / s10b_tests 互补，聚焦仍缺的确定性错误臂：
//! - `Default` impl + 命令表无重复
//! - `pending` 的 enumerate 错误臂：box_root 预置为普通文件 → walk 的
//!   read_dir 失败 → `enumerate box: ...`
//! - `set_network` 的 Sandboxie.ini 重写失败臂（base 目录被文件占位 →
//!   create_dir_all 失败）与 Start.exe /reload 缺失 spawn 失败臂（ini 写
//!   成功但 runtime 未就绪 → spawn Err）
//!
//! 结构性豁免（见报告）：run_cli_subcmd 全臂（spawn 的是测试二进制自身，
//! libtest 会把 "sandbox start" 当过滤器递归跑测试——真引擎路径本就豁免）、
//! update_executor/current_executor 的 global ConfigStore 分支（进程级
//! 状态，测试进程不装配）、overview 非 Windows 分支（cfg! 常量假）、
//! install_7z / install_sandboxie（真下载）、open_box/open_explorer
//! （真开资源管理器窗口 + 服务就绪门）、self_test 后端存在分支（Windows
//! 无用户态后端 → 常量不可达）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// CLI/no-store 模式下 update_executor 要求 config.json 存在（读合并写）。
fn agt_write_config_json(home: &std::path::Path) {
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({ "executor": { "allow_network": false } }).to_string(),
    )
    .unwrap();
}

#[test]
fn agt_default_impl_and_command_table() {
    let h = SandboxHandler;
    assert_eq!(h.module_name(), "sandbox");
    let cmds = h.commands();
    assert!(cmds.contains(&"overview"));
    assert!(cmds.contains(&"self_test"));
    assert!(cmds.contains(&"set_network"));
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "duplicate command in table");
}

#[tokio::test]
async fn agt_pending_enumerate_error_when_box_root_is_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = nemesis_sandbox::SandboxPaths::new(dir.path());
    std::fs::create_dir_all(paths.box_root.parent().unwrap()).unwrap();
    std::fs::write(&paths.box_root, b"not a dir").unwrap();
    let ctx = agt_make_ctx(&dir);
    let err = SandboxHandler::new()
        .handle_cmd("pending", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("enumerate box"), "err: {err}");
}

#[tokio::test]
async fn agt_set_network_ini_rewrite_and_reload_spawn_failure_arms() {
    let h = SandboxHandler::new();
    let data = serde_json::json!({ "enabled": true });

    // ① ini 写失败：base 目录被文件占位 → create_dir_all 失败
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("workspace").join("tools").join("sandboxie");
    std::fs::create_dir_all(base.parent().unwrap()).unwrap();
    std::fs::write(&base, b"not a dir").unwrap();
    agt_write_config_json(dir.path());
    let ctx = agt_make_ctx(&dir);
    let err = h
        .handle_cmd("set_network", Some(data.clone()), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("rewrite Sandboxie.ini"), "err: {err}");

    // ② ini 写成功但 Start.exe 缺失 → /reload spawn 失败（无副作用：
    //    Command::new(不存在路径).spawn() 立即 Err）
    let dir2 = tempfile::tempdir().unwrap();
    agt_write_config_json(dir2.path());
    let ctx2 = agt_make_ctx(&dir2);
    let err2 = h
        .handle_cmd("set_network", Some(data), &ctx2)
        .await
        .unwrap_err();
    assert!(err2.contains("spawn Start.exe /reload"), "err2: {err2}");
    // Sandboxie.ini 已落盘（写成功在 spawn 之前）
    let ini = dir2
        .path()
        .join("workspace")
        .join("tools")
        .join("sandboxie")
        .join("Sandboxie.ini");
    assert!(ini.exists(), "ini must be written before reload");
    let content = std::fs::read_to_string(ini).unwrap();
    assert!(content.contains("AllowNetworkAccess=y"), "{content}");
}

// ---------------------------------------------------------------------------
// Wave5 批次：Default impl、current_executor 的 CLI fallback 读失败臂、
// self_test 的 Windows 无后端诚实臂（supported:false，不 spawn 子进程）。
// ---------------------------------------------------------------------------

/// `Default` impl 三行（trait 转发 new）。
#[test]
fn w5_default_impl_delegates_to_new() {
    // 经泛型间接调用：单元结构体直呼 `Default::default()` 会被
    // clippy::default_constructed_unit_structs 建议改成字面量（那就执行不到
    // default() 的产品行了）；间接一层 lint 干净，且真实走 trait 转发。
    fn via_default<H: Default>() -> H {
        H::default()
    }
    let _handler = via_default::<SandboxHandler>();
}

/// CLI fallback（无 global store）：home 下无 config.json → 读失败 →
/// 四开关全默认 false（142-144 臂），不 panic。
#[test]
fn w5_current_executor_missing_config_defaults_to_all_false() {
    if nemesis_config::global().is_some() {
        eprintln!("skip: process-global ConfigStore installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("no-config-here"); // 故意不存在
    let e = current_executor(&home);
    assert!(
        !e.enabled && !e.sandbox && !e.allow_network && !e.strict,
        "{e:?}"
    );
}

/// Windows 无用户态后端 → self_test 诚实报 supported:false，不 spawn
/// 探针子进程（620-632）。
#[cfg(windows)]
#[tokio::test]
async fn w5_selftest_reports_unsupported_without_userland_backend() {
    // 若机器装了 landlock/bwrap 等价物（Windows 上没有）则本测试前提失效，
    // 诚实跳过；正常 Windows 机 detect_backend() == None。
    if nemesis_sandbox::backend::detect_backend().is_some() {
        eprintln!("skip: userland backend present, unsupported arm unreachable");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_make_ctx(&dir);
    let out = SandboxHandler
        .handle_cmd("self_test", None, &ctx)
        .await
        .expect("self_test must not error on empty backend")
        .expect("payload");
    assert_eq!(out["supported"], false, "{out}");
    assert!(out["backend"].is_null(), "{out}");
}

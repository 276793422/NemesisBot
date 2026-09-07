//! L1（devtool-upgrade 阶段 6）— WSAPI 命令注册表测试。
//!
//! 三件事：
//! 1. 结构不变量——31 个注册模块（含 feature 门控的 8 个，按 cfg 断言）
//!    清单非空、模块内无重复、锚点命令在位；
//! 2. `system.commands` dispatch 链路（OnceLock 快照 → handler → JSON）；
//! 3. 文档生成——`docs/INFO/wsapi-commands.md`（docs/ 已 gitignore，测试
//!    写仓库树无副作用泄漏）；安全子集 dispatch 冒烟（只挑无参只读命令；
//!    依赖 agent loop 的命令断言其诚实报错路径）。
//!
//! ⚠️ 安全子集选择纪律：只 dispatch **无副作用**命令。history_reindex /
//! spill_cleanup / shop.refresh / estop.trigger 这类会动真实状态的一律不测。

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext, WsRouter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness（AppState 全量字面量——与 m5_session_usage_tests 同款双臂）
// ---------------------------------------------------------------------------

fn make_ctx(dir: &tempfile::TempDir, with_config: bool) -> RequestContext {
    if with_config {
        let config = nemesis_config::Config::default();
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }
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
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 独立 router 跑 register_all → 快照（同时把进程级 OnceLock 发布出来，
/// 供 `system.commands` dispatch 测试用——内容幂等，先写胜出无害）。
fn build_registry() -> Vec<(String, Vec<&'static str>)> {
    let mut router = WsRouter::new();
    super::register_all(&mut router);
    router.commands_registry()
}

fn cmds_of<'a>(reg: &'a [(String, Vec<&'static str>)], module: &str) -> &'a [&'static str] {
    reg.iter()
        .find(|(m, _)| m == module)
        .map(|(_, c)| c.as_slice())
        .unwrap_or(&[])
}

/// 无 feature 闸的 23 个模块（register_all 的无条件注册段）。
const UNCONDITIONAL_MODULES: &[&str] = &[
    "system", "estop", "approval", "question", "chat", "config", "models", "channels", "identity",
    "tools", "skills", "mcp", "tasks", "coding", "hooks", "commands", "fs", "plugins", "board",
    "logs", "agent", "persona", "sessions",
];

// ---------------------------------------------------------------------------
// 结构不变量
// ---------------------------------------------------------------------------

#[test]
fn registry_all_modules_nonempty_no_dupes() {
    let reg = build_registry();
    // 23 个无条件模块必须全在；8 个 feature 门控模块按 cfg 计入下限。
    // （cfg! 的 feature 参数必须是字面量，无法循环展开——逐个列出。）
    let gated_on = [
        cfg!(feature = "cluster"),
        cfg!(feature = "voice"),
        cfg!(feature = "workflow"),
        cfg!(feature = "memory"),
        cfg!(feature = "security"), // 同时带 scanner，两个模块
        cfg!(feature = "sandbox"),
        cfg!(feature = "forge"),
    ];
    let expected_floor = UNCONDITIONAL_MODULES.len()
        + gated_on.iter().filter(|b| **b).count()
        + if cfg!(feature = "security") { 1 } else { 0 };
    assert!(
        reg.len() >= expected_floor,
        "registry modules {} < floor {expected_floor}: {:?}",
        reg.len(),
        reg.iter().map(|(m, _)| m).collect::<Vec<_>>()
    );
    for (module, cmds) in &reg {
        assert!(
            !cmds.is_empty(),
            "module {module} has empty commands() list"
        );
        let mut seen = std::collections::HashSet::new();
        for c in cmds {
            assert!(seen.insert(*c), "module {module} duplicates command {c}");
        }
    }
    for m in UNCONDITIONAL_MODULES {
        assert!(
            reg.iter().any(|(name, _)| name == m),
            "unconditional module {m} missing from registry"
        );
    }
}

#[test]
fn registry_anchor_commands_present() {
    let reg = build_registry();
    assert!(cmds_of(&reg, "system").contains(&"commands"));
    assert!(cmds_of(&reg, "system").contains(&"version"));
    assert!(cmds_of(&reg, "estop").contains(&"trigger"));
    assert!(cmds_of(&reg, "sessions").contains(&"rewind_to_message"));
    assert!(cmds_of(&reg, "sessions").contains(&"redo"));
    assert!(cmds_of(&reg, "logs").contains(&"history_search"));
    // feature 门控模块锚点
    if cfg!(feature = "workflow") {
        let w = cmds_of(&reg, "workflow");
        assert_eq!(w.len(), 19);
        assert!(w.contains(&"set_chat_password"));
    }
    if cfg!(feature = "voice") {
        assert_eq!(cmds_of(&reg, "voice").len(), 39);
    }
    if cfg!(feature = "cluster") {
        assert!(cmds_of(&reg, "cluster").contains(&"node.update_identity"));
    }
    if cfg!(feature = "security") {
        assert!(cmds_of(&reg, "security").contains(&"config.get"));
        assert!(cmds_of(&reg, "scanner").contains(&"config.get"));
    }
    if cfg!(feature = "sandbox") {
        assert!(cmds_of(&reg, "sandbox").contains(&"commit"));
    }
    if cfg!(feature = "forge") {
        assert!(cmds_of(&reg, "forge").contains(&"reflect"));
    }
    if cfg!(feature = "memory") {
        assert!(cmds_of(&reg, "memory").contains(&"entries.search"));
    }
}

// ---------------------------------------------------------------------------
// system.commands dispatch 链路
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_commands_returns_registry_via_dispatch() {
    let reg = build_registry();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    let out = super::system::SystemHandler
        .handle_cmd("commands", None, &ctx)
        .await
        .expect("system.commands should succeed");
    let out = out.expect("system.commands returns payload");
    let modules = out["modules"].as_array().expect("modules array");
    assert_eq!(modules.len(), reg.len());
    let total_cmds = out["total_cmds"].as_u64().unwrap() as usize;
    let expected_total: usize = reg.iter().map(|(_, c)| c.len()).sum();
    assert_eq!(total_cmds, expected_total);
    // 注册表经 dispatch 可见 = OnceLock 发布链路通。
    assert!(
        total_cmds > 100,
        "suspiciously small registry: {total_cmds}"
    );
}

// ---------------------------------------------------------------------------
// 文档生成（docs/INFO/wsapi-commands.md）
// ---------------------------------------------------------------------------

#[test]
fn docs_generation_writes_wsapi_commands_md() {
    let reg = build_registry();
    let total: usize = reg.iter().map(|(_, c)| c.len()).sum();
    let mut md = String::new();
    md.push_str("# WSAPI 命令注册表（自动生成）\n\n");
    md.push_str(
        "> 由 `crates/nemesis-web/src/handlers/l1_tests.rs::\
docs_generation_writes_wsapi_commands_md` 从 `ModuleHandler::commands()` \
静态清单生成——**勿手改**。改任何 handler 的命令臂后重跑 \
`cargo test -p nemesis-web l1_docs` 刷新本文件。\n\n",
    );
    md.push_str(&format!("共 {} 个模块 / {} 条命令。\n\n", reg.len(), total));
    md.push_str("| module | commands | count |\n|---|---|---|\n");
    for (module, cmds) in &reg {
        md.push_str(&format!(
            "| {} | {} | {} |\n",
            module,
            cmds.join(", "),
            cmds.len()
        ));
    }
    let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/INFO");
    std::fs::create_dir_all(&docs).unwrap();
    let path = docs.join("wsapi-commands.md");
    std::fs::write(&path, &md).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("| system | version, status, commands |"));
    assert!(written.contains(&format!("{} 条命令", total)));
}

// ---------------------------------------------------------------------------
// 安全子集 dispatch 冒烟
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_safe_readonly_subset_ok() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true); // 写入最小 config.json → load_config 可解析

    // (module, handler) 只读无参命令，全部必须 Ok。
    macro_rules! ok_cmd {
        ($handler:expr, $cmd:expr) => {
            $handler
                .handle_cmd($cmd, None, &ctx)
                .await
                .unwrap_or_else(|e| panic!("{} must succeed, got: {}", $cmd, e))
                .expect("expected payload");
        };
    }
    ok_cmd!(super::system::SystemHandler, "version");
    ok_cmd!(super::system::SystemHandler, "status");
    ok_cmd!(super::system::SystemHandler, "commands");
    ok_cmd!(super::estop::EstopHandler, "status");
    ok_cmd!(super::channels::ChannelsHandler::new(), "list");
    ok_cmd!(super::models::ModelsHandler::new(), "list");
    ok_cmd!(super::config::ConfigHandler::new(), "get");
}

#[tokio::test]
async fn dispatch_dependent_commands_report_honest_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false); // 无 agent loop / responder 槽

    // 这些命令依赖 agent loop 装配——headless fixture 下必须**诚实报错**
    //（而不是 Ok 假成功或 panic）。断言的是报错路径本身。
    for (handler, cmd) in [
        (
            &super::approval::ApprovalHandler as &dyn ModuleHandler,
            "pending",
        ),
        (
            &super::question::QuestionHandler as &dyn ModuleHandler,
            "pending",
        ),
        (&super::tools::ToolsHandler as &dyn ModuleHandler, "list"),
    ] {
        let err = handler
            .handle_cmd(cmd, None, &ctx)
            .await
            .expect_err("expected honest error without agent loop");
        assert!(
            err.contains("agent loop not running") || err.contains("agent not running"),
            "{cmd} error should name the missing dependency, got: {err}"
        );
    }
}

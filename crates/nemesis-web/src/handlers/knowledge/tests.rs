//! `knowledge.get/set` handler 测试 — resolve_scope 三分路由 + 读写冒烟。
//!
//! 三分顺序（与 `resolve_session_loop` 同款判定，键空间不相交）：agents
//! 纯字符串解析在先 → projects bridge 命中即诚实拒绝（v1 不支持）→ 主槽
//! workspace config 兜底。bridge 是进程级共享槽——触碰它的用例持
//! BRIDGE_TEST_LOCK 串行 + assert 前先摘桥（fs/tests.rs 同款纪律）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::knowledge::KnowledgeHandler;
use crate::handlers::projects::{
    BRIDGE_TEST_LOCK, ProjectInfo, ProjectsBridge, set_projects_bridge_for_test,
};
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(ws_dir: &std::path::Path) -> RequestContext {
    let ws = ws_dir.to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
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
        estop_audit: None,
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
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// resolve_scope 三分路由

#[test]
fn resolve_scope_main_when_no_session() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let (scope, path) = resolve_scope(&ctx, None).unwrap();
    assert_eq!(scope, "main");
    assert_eq!(
        path,
        dir.path().join("config").join("config.knowledge.json")
    );
}

#[test]
fn resolve_scope_main_when_empty_or_plain_session() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    // 空串与普通会话键（无 agents: 前缀、无 bridge 归属）都落主槽。
    for sid in ["", "agent:main:session:abc"] {
        let (scope, _) = resolve_scope(&ctx, Some(sid)).unwrap();
        assert_eq!(scope, "main", "session_id={sid:?} 应落主槽");
    }
}

#[test]
fn resolve_scope_agent_arm() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let (scope, path) = resolve_scope(&ctx, Some("agents:alpha:session:s1")).unwrap();
    assert_eq!(scope, "agent");
    assert_eq!(
        path,
        dir.path()
            .join("agents")
            .join("alpha")
            .join("home")
            .join("config")
            .join("config.knowledge.json")
    );
}

#[test]
fn resolve_scope_workspace_missing_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx(dir.path());
    ctx.workspace = None;
    let err = resolve_scope(&ctx, None).unwrap_err();
    assert!(err.contains("workspace not available"));
}

/// 哨兵归属桥：只命中 `__kb_sentinel__` 会话键，其余全 None。
struct OwnerBridge;

impl ProjectsBridge for OwnerBridge {
    fn list(&self) -> Vec<ProjectInfo> {
        vec![]
    }
    fn create(&self, _name: &str, _path: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn remove(&self, _id: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn rename(&self, _id: &str, _n: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn set_trust(&self, _id: &str, _t: bool) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn add_dir(&self, _id: &str, _d: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn remove_dir(&self, _id: &str, _d: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn owner_of(&self, s: &str) -> Option<String> {
        (s == "agent:main:session:__kb_sentinel__").then(|| "p-kb".to_string())
    }
    fn loop_for_session(&self, _id: &str) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        None
    }
    fn project_path(&self, _id: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn bind_session(&self, _s: &str, _id: &str) -> Result<(), String> {
        Err("noop".to_string())
    }
    fn forget_session(&self, _s: &str) {}
}

#[test]
fn resolve_scope_project_arm_rejects() {
    let _serial = BRIDGE_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    // 未注册 bridge：哨兵键也落主槽（三分第 2 臂不命中）。
    let (scope, _) = resolve_scope(&ctx, Some("agent:main:session:__kb_sentinel__")).unwrap();
    assert_eq!(scope, "main");
    // 注册 bridge：哨兵键命中项目归属 → 诚实拒绝（不静默落主槽）。
    set_projects_bridge_for_test(Some(Arc::new(OwnerBridge)));
    let result = resolve_scope(&ctx, Some("agent:main:session:__kb_sentinel__"));
    // assert 前先摘桥（进程级共享槽——即使 assert 失败也不污染兄弟测试）。
    set_projects_bridge_for_test(None);
    let err = result.unwrap_err();
    assert!(err.contains("项目会话暂不支持"), "实际错误：{err}");
}

// get / set 冒烟（走 handler 全链）

/// 平台中立绝对路径夹具（nemesis-config knowledge 测试同款）：validator 用
/// `Path::is_absolute()`（平台语义——`C:/` 形态在 Linux 无盘符前缀组件 =
/// 相对路径，被诚实拒绝）。BUG-20261128-008 四连红根因 = 夹具硬编码
/// Windows 路径而非产品代码；按平台构造真实绝对路径，不用 cfg 门控
/// （会把 Linux 侧校验覆盖归零）。
fn abs_root(tag: &str) -> String {
    if cfg!(windows) {
        format!("C:/kb_{tag}")
    } else {
        format!("/tmp/kb_{tag}")
    }
}

#[tokio::test]
async fn get_returns_default_config() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let out = KnowledgeHandler
        .handle_cmd("get", Some(serde_json::json!({ "session_id": "" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["scope"], "main");
    assert_eq!(out["config"]["enabled"], false);
    assert_eq!(out["config"]["pairs"], serde_json::json!([]));
    assert!(
        out["config_path"]
            .as_str()
            .unwrap()
            .ends_with("config.knowledge.json")
    );
}

#[tokio::test]
async fn set_roundtrip_and_readback() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let cfg = serde_json::json!({
        "enabled": true,
        "pairs": [{
            "code_root": abs_root("proj-a"),
            "info_root": abs_root("proj-a-info"),
            "tiangong_load": "full",
        }],
    });
    let out = KnowledgeHandler
        .handle_cmd(
            "set",
            Some(serde_json::json!({ "session_id": "", "config": cfg })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["saved"], true);

    // 读回一致。
    let back = KnowledgeHandler
        .handle_cmd("get", Some(serde_json::json!({ "session_id": "" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back["config"]["enabled"], true);
    assert_eq!(back["config"]["pairs"].as_array().unwrap().len(), 1);
    assert_eq!(back["config"]["pairs"][0]["tiangong_load"], "full");
    // 配置文件确实落盘。
    assert!(
        dir.path()
            .join("config")
            .join("config.knowledge.json")
            .exists()
    );
}

#[tokio::test]
async fn set_rejects_relative_code_root() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let cfg = serde_json::json!({
        "enabled": true,
        "pairs": [{
            "code_root": "relative/proj",
            "info_root": abs_root("proj"),
        }],
    });
    let err = KnowledgeHandler
        .handle_cmd(
            "set",
            Some(serde_json::json!({ "session_id": "", "config": cfg })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("绝对路径"), "实际错误：{err}");
    // 拒绝即不落盘。
    assert!(
        !dir.path()
            .join("config")
            .join("config.knowledge.json")
            .exists()
    );
}

#[tokio::test]
async fn unknown_command_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let err = KnowledgeHandler
        .handle_cmd("delete", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown command: knowledge.delete"));
}

/// 第十一轮横扫（2026-11-27）：恶意 session_key 的 id 段（穿越形态）经
/// nemesis-path 路径层消毒 confine——resolve_scope 产出的 config 路径恒在
/// workspace/agents 内（此前纯字符串解析的 id 直拼 join，配合
/// write_file_atomic 的 create_dir_all 可把 set 写出 workspace）。
#[test]
fn resolve_scope_confines_traversal_agent_id() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let (scope, path) =
        resolve_scope(&ctx, Some(r"agents:..\..\evil:session:x")).expect("agent 臂命中");
    assert_eq!(scope, "agent");
    assert!(
        path.starts_with(dir.path().join("agents")),
        "config 路径必须 confine 在 workspace/agents 内: {}",
        path.display()
    );
    // 消毒后无 .. 组件（逐组件核查）。
    let rel = path.strip_prefix(dir.path()).unwrap();
    assert!(
        rel.components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "消毒后不得含穿越组件: {}",
        path.display()
    );
}

//! editor.rs AGT 覆盖率批次（2026-09-25）：refresh_roots 的 bridge 循环体
//! （56-58）——主 workspace 之外再吃进项目 registry 全部项目根。用假
//! ProjectsBridge（list 返回一个真实存在的项目目录）+ 真 EditorAccessState
//! 走 handle_cmd("get")，再从 ABAC 侧验证 root 真的注入了：项目内 FileWrite
//! 短路 Allowed（editor_access:full），项目外写删族回落 None（开关二未开）。
//!
//! 进程级双槽（PROJECTS_BRIDGE / EDITOR_ACCESS）共享 → 全文件持两把测试串
//! 行锁；async 测试持 parking_lot 锁跨 await 沿 editor/tests.rs 家规。
#![allow(clippy::await_holding_lock)]

use super::EDITOR_TEST_LOCK;
use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::projects::{
    BRIDGE_TEST_LOCK, ProjectInfo, ProjectsBridge, install_projects_bridge,
    set_projects_bridge_for_test,
};
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 单项目假 bridge：只 list 是真的，其余臂本测试不触（诚实 Err / None）。
struct AgtOneProjectBridge {
    project: ProjectInfo,
}

impl ProjectsBridge for AgtOneProjectBridge {
    fn list(&self) -> Vec<ProjectInfo> {
        vec![self.project.clone()]
    }
    fn create(&self, _name: &str, _path: &str) -> Result<ProjectInfo, String> {
        Err("agt: not implemented".to_string())
    }
    fn remove(&self, _project_id: &str) -> Result<ProjectInfo, String> {
        Err("agt: not implemented".to_string())
    }
    fn rename(&self, _project_id: &str, _new_name: &str) -> Result<ProjectInfo, String> {
        Err("agt: not implemented".to_string())
    }
    fn owner_of(&self, _session_key: &str) -> Option<String> {
        None
    }
    fn loop_for_session(&self, _project_id: &str) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        None
    }
    fn project_path(&self, _project_id: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn bind_session(&self, _session_key: &str, _project_id: &str) -> Result<(), String> {
        Err("agt: not implemented".to_string())
    }
    fn forget_session(&self, _session_key: &str) {}
}

fn agt_ctx(workspace: Option<String>) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: workspace.clone(),
        home: None,
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
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "agt-editor".to_string(),
        chat_id: "web:agt-editor".to_string(),
        workspace,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn agt_get_refreshes_roots_from_bridge_and_abac_sees_project_root() {
    let _bridge_guard = BRIDGE_TEST_LOCK.lock();
    let _editor_guard = EDITOR_TEST_LOCK.lock();

    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("agt-proj");
    std::fs::create_dir_all(&proj).unwrap();
    let bridge = Arc::new(AgtOneProjectBridge {
        project: ProjectInfo {
            id: "agt-p1".to_string(),
            name: "AGT 项目".to_string(),
            path: proj.to_string_lossy().to_string(),
            created_at: "2026-09-25T00:00:00Z".to_string(),
            running: false,
        },
    });
    install_projects_bridge(bridge);

    let access = nemesis_security::editor_access::EditorAccessState::new();
    install_editor_access(access.clone());
    access.set_flags(true, false);

    let ctx = agt_ctx(Some(dir.path().to_string_lossy().to_string()));
    let resp = EditorHandler.handle_cmd("get", None, &ctx).await.unwrap();
    let resp = resp.expect("get 必有响应体");
    assert_eq!(resp["full_access"], serde_json::Value::Bool(true));

    // ABAC 侧验证：refresh_roots 已把项目根注入 → 项目内 FileWrite 短路放行。
    let inside = access.evaluate(
        nemesis_security::types::OperationType::FileWrite,
        &proj.join("f.txt").to_string_lossy(),
    );
    let (decision, reason, rule) = inside.expect("项目内写删必须短路");
    assert_eq!(decision, nemesis_security::types::SecurityDecision::Allowed);
    assert!(
        reason.contains("inside workspace roots"),
        "reason: {reason}"
    );
    assert_eq!(rule, "editor_access:full");

    // 项目外写删族：开关二未开 → 回落原判定链（None）。注意 refresh_roots
    // 同时注入了主 workspace（dir）与项目根（dir/agt-proj）——「外」目标必须
    // 在两者之外（系统临时根下，与测试 tempdir 必不相交）。
    let outside = access.evaluate(
        nemesis_security::types::OperationType::FileWrite,
        &std::env::temp_dir()
            .join("agt-editor-outside")
            .join("note.txt")
            .to_string_lossy(),
    );
    assert!(outside.is_none(), "项目外写删在 ext 未开时必须回落");

    set_projects_bridge_for_test(None);
    set_editor_access_for_test(None);
}

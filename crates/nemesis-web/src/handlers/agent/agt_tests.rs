//! agent.rs AGT 覆盖率批次（2026-09-25）。与 tests / s10b_tests 互补，
//! 聚焦仍缺的确定性臂：
//! - `retry_status` 全函数：无 loop 的诚实 unavailable 臂（resolve Err）+
//!   有 loop 无重试的 `{available, retrying:false}` 臂
//! - cancel/rewind/checkpoints 的 `session_id` 分派臂（项目会话归属解析
//!   走 `resolve_session_loop`，无 bridge 时回落主槽）
//! - `start` 的模型热切联动臂（projects bridge 在位 →
//!   `reload_provider_all`）+ `update_model_info` 的 config.json 缺失臂
//!   （load_config Err → if-let 落空）
//!
//! 结构性豁免（见报告）：retry_status 的 `retrying:true` 快照臂——需要
//! 限流重试环在退避等待中（真实 429 turn 的中间态），快照写侧
//! `set_rate_limit_status` 是 nemesis-agent pub(crate)，web 测试无法种子
//! 化；agent 侧 rate_limit_retry_tests 已覆盖该语义。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::projects::{
    BRIDGE_TEST_LOCK, ProjectInfo, ProjectsBridge, install_projects_bridge,
    set_projects_bridge_for_test,
};
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::r#loop::{AgentLoop, LlmProvider, LlmResponse};
use nemesis_agent::types::AgentConfig;
use nemesis_services::bot_service::{AgentLoopService, LifecycleService};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

struct AgtNoopProvider;

#[async_trait::async_trait]
impl LlmProvider for AgtNoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// start() 成功臂的最小 service 替身（同 tests.rs 的 Mock 形态）。
struct AgtMockService {
    running: AtomicBool,
}
impl LifecycleService for AgtMockService {
    fn start(&self) -> Result<(), String> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
impl AgentLoopService for AgtMockService {}

/// start 热切联动测试的最小 bridge 替身——只验证 `reload_provider_all`
/// 默认实现被调（不炸），其余方法诚实空实现。
struct AgtStubBridge;
impl ProjectsBridge for AgtStubBridge {
    fn list(&self) -> Vec<ProjectInfo> {
        Vec::new()
    }
    fn create(&self, _name: &str, _path: &str) -> Result<ProjectInfo, String> {
        Err("not wired".to_string())
    }
    fn remove(&self, _project_id: &str) -> Result<ProjectInfo, String> {
        Err("not wired".to_string())
    }
    fn rename(&self, _project_id: &str, _new_name: &str) -> Result<ProjectInfo, String> {
        Err("not wired".to_string())
    }
    fn owner_of(&self, _session_key: &str) -> Option<String> {
        None
    }
    fn loop_for_session(&self, _project_id: &str) -> Option<Arc<AgentLoop>> {
        None
    }
    fn project_path(&self, _project_id: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn bind_session(&self, _session_key: &str, _project_id: &str) -> Result<(), String> {
        Err("not wired".to_string())
    }
    fn forget_session(&self, _session_key: &str) {}
}

fn agt_ctx(
    al: Option<Arc<AgentLoop>>,
    svc: Option<Arc<dyn AgentLoopService>>,
    home: Option<String>,
) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("agt-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: svc,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(al)),
        cluster: None,
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
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: None,
        home,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn agt_live_loop() -> Arc<AgentLoop> {
    Arc::new(AgentLoop::new(
        Box::new(AgtNoopProvider),
        AgentConfig::default(),
    ))
}

// ---------------------------------------------------------------------------
// retry_status：Err（无 loop）+ Ok/None（有 loop 无重试中）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_retry_status_unavailable_then_ok_no_retry() {
    let handler = AgentHandler;

    // ① 无 loop：resolve_session_loop 落空 → available:false + 诚实 error。
    //    显式 sid 与缺省 sid 各走一遍（sanitize 臂 + legacy 臂）。
    let ctx = agt_ctx(None, None, None);
    for has_sid in [true, false] {
        let data = has_sid.then(|| serde_json::json!({ "session_id": "agt retry 1" }));
        let out = handler
            .handle_cmd("retry_status", data.clone(), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out["available"], false, "{out}");
        assert_eq!(out["retrying"], false);
        assert_eq!(out["error"], "agent loop not running");
        // 该失败形态无 session_key 字段（与 inbox_status 的 Err 形态不同）。
        assert!(out.get("session_key").is_none(), "{out}");
    }

    // ② 有 loop：快照缺席 → available:true / retrying:false（165-168 臂）。
    let ctx2 = agt_ctx(Some(agt_live_loop()), None, None);
    for data in [
        Some(serde_json::json!({ "session_id": "agt-rs" })),
        Some(serde_json::json!({})),
    ] {
        let out = handler
            .handle_cmd("retry_status", data, &ctx2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out["available"], true, "{out}");
        assert_eq!(out["retrying"], false);
        assert!(out.get("error").is_none(), "成功臂不带 error 字段: {out}");
    }
}

// ---------------------------------------------------------------------------
// cancel / rewind / checkpoints 的 session_id 分派臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_session_scoped_cancel_rewind_checkpoints() {
    let handler = AgentHandler;
    let ctx = agt_ctx(Some(agt_live_loop()), None, None);

    // cancel 带 session_id：归属解析走 resolve_session_loop（无 bridge →
    // 主槽命中）→ 计数 0（221-229 + 233-237）。
    let out = handler
        .handle_cmd(
            "cancel",
            Some(serde_json::json!({ "session_id": "agt-cancel" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], 0, "{out}");

    // cancel 带 session_id 但主槽空 → `?` 透传 resolve Err（229 臂）。
    let ctx_empty = agt_ctx(None, None, None);
    let err = handler
        .handle_cmd(
            "cancel",
            Some(serde_json::json!({ "session_id": "agt-x" })),
            &ctx_empty,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "agent loop not running");

    // rewind 带 session_id：live loop 无 checkpoint store → Err 透传
    // （262-270 臂）。
    let err = handler
        .handle_cmd(
            "rewind",
            Some(serde_json::json!({ "turn": 1, "session_id": "agt-rw" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("checkpoint store not attached"), "{err}");

    // checkpoints 带 session_id：列表走项目归属解析（300-308 臂）。
    let out = handler
        .handle_cmd(
            "checkpoints",
            Some(serde_json::json!({ "session_id": "agt-cp" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(out["checkpoints"].as_array().unwrap().is_empty(), "{out}");
}

// ---------------------------------------------------------------------------
// start：bridge 热切联动 + update_model_info 的 config 缺失臂
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // 持 BRIDGE_TEST_LOCK 串行化共享 bridge 槽（测试纪律，故意跨 await）
async fn agt_start_with_bridge_reload_and_config_load_fail() {
    // bridge 槽是进程级共享态——持既有 BRIDGE_TEST_LOCK 串行（同
    // projects_tests 的 BridgeGuard 纪律），结束复位 None。
    let _lock = BRIDGE_TEST_LOCK.lock();
    install_projects_bridge(Arc::new(AgtStubBridge));

    // home 有 config.json 但内容非法 → load_config Err → update_model_info
    // 的 if-let 落空臂（349），AppState 模型字段保持原值。（注意：config
    // 文件**缺失**时 load_config 回落默认配置并不 Err——模型字段会被默认
    // 解析覆盖，不能用缺文件触发这条臂。）
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), b"{ not valid json").unwrap();
    let ctx = agt_ctx(
        None,
        Some(Arc::new(AgtMockService {
            running: AtomicBool::new(false),
        })),
        Some(dir.path().to_string_lossy().to_string()),
    );
    let out = AgentHandler
        .handle_cmd("start", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["started"], true, "{out}");
    // update_model_info 早落空：模型字段不被清空。
    assert_eq!(*ctx.state.model_name.lock(), "agt-model");
    assert!(!ctx.state.model_has_key.load(Ordering::SeqCst));

    // update_model_info 直接调用同样覆盖缺失臂（home 有但 config.json 无）。
    super::update_model_info(&ctx);

    set_projects_bridge_for_test(None);
}

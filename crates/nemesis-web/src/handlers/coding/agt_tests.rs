//! coding.rs AGT 覆盖率批次（2026-09-25）。与 tests 互补，聚焦 lsp_install
//! 的语言解析后段：
//! - needs_interactive 诚实拒绝臂（仅非 Windows 存在 sudo 型命令——
//!   Windows 全目录非交互，测试按平台双态断言）
//! - Plan 模式预检拒绝臂（loop 在位但 mode=Plan → 针对性文案，不进
//!   dispatch）
//! - dispatch 全身（tool_call 构造 + 独立 RequestContext + 结果回显）：
//!   裸 AgentLoop（未注册任何工具）→ dispatch 在「Unknown tool」处短路，
//!   **不执行任何安装命令**——结果串内容不固化（安全闸分段返回文案各异，
//!   只断言是 String）。
//!
//! 结构性豁免（见报告）：232 的 load_live 全局 store 命中臂（装全局
//! ConfigStore 会劫持并行测试的 home 隔离，同 models.rs 101 的裁决）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext as Ctx};
use nemesis_agent::r#loop::{AgentLoop, LlmProvider, LlmResponse};
use nemesis_agent::types::AgentConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct AgtNoopProvider;

#[async_trait::async_trait]
impl LlmProvider for AgtNoopProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
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

fn agt_ctx(al: Option<Arc<AgentLoop>>) -> Ctx {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
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
    Ctx {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn agt_live_loop() -> Arc<AgentLoop> {
    // 裸 loop：不 register 任何工具 → exec 查表必落 Unknown tool 短路臂。
    Arc::new(AgentLoop::new(
        Box::new(AgtNoopProvider),
        AgentConfig::default(),
    ))
}

/// needs_interactive 拒绝臂 + 平台目录双态。
#[tokio::test]
async fn agt_lsp_install_interactive_rejection_is_platform_conditional() {
    let handler = CodingHandler;
    // 找一个平台上的交互型安装命令（Linux = C 的 sudo apt）。
    let interactive = nemesis_lsp::registry::SERVERS
        .iter()
        .map(|s| s.lang)
        .find(|l| nemesis_lsp::install::install_command(*l).needs_interactive);

    match interactive {
        Some(lang) => {
            let ctx = agt_ctx(None);
            let err = handler
                .handle_cmd(
                    "lsp_install",
                    Some(serde_json::json!({ "lang": format!("{lang:?}") })),
                    &ctx,
                )
                .await
                .unwrap_err();
            assert!(
                err.contains("需要交互终端"),
                "honest copy-install rejection: {err}"
            );
            assert!(
                err.contains(&nemesis_lsp::install::install_command(lang).display),
                "must echo the copyable command: {err}"
            );
        }
        None => {
            // Windows：全目录非交互（winget/cmd /C 均可静默）——拒绝臂在本
            // 平台结构性不可达；退而断言目录诚实（每个命令都可一键）。
            for s in nemesis_lsp::registry::SERVERS.iter() {
                assert!(
                    !nemesis_lsp::install::install_command(s.lang).needs_interactive,
                    "windows catalog must be non-interactive"
                );
            }
        }
    }
}

/// Plan 模式预检：loop 在位但 mode=Plan → 针对性拒绝（不进 dispatch）。
#[tokio::test]
async fn agt_lsp_install_rejected_in_plan_mode() {
    let al = agt_live_loop();
    al.set_mode_with_event(nemesis_agent::types::AgentMode::Plan, "", "");
    let ctx = agt_ctx(Some(al));
    let err = CodingHandler
        .handle_cmd(
            "lsp_install",
            Some(serde_json::json!({ "lang": "Rust" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "{err}");
    assert!(err.contains("/build"), "{err}");
}

/// dispatch 全身：裸 loop（无 exec 工具）→ handle_tool_call 返回错误串
/// （Unknown tool / 安全闸分段拒绝——绝不执行安装命令），handler 回显
/// lang/command/result 三元组。
#[tokio::test]
async fn agt_lsp_install_dispatches_exec_and_echoes_result() {
    let _home = crate::test_home::lock_home();
    let ctx = agt_ctx(Some(agt_live_loop()));
    let out = CodingHandler
        .handle_cmd(
            "lsp_install",
            Some(serde_json::json!({ "lang": "Rust" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["lang"], "Rust", "{out}");
    assert_eq!(out["command"], "rustup component add rust-analyzer");
    assert!(
        out["result"].is_string(),
        "dispatch result must be a string: {out}"
    );
}

//! BUG-20261113-156 回归锁：workbench WSAPI 命令名形态契约。
//!
//! 旧形态：commands() 与 match 臂用全名（"workbench.get"/"workbench.respond"），
//! 而 router 把 `msg.cmd` **原样**传进 handle_cmd、前端 `request('workbench',
//! 'get')` 发的是裸名——全部请求落 unknown 分支，工作台页面取数与动作全灭。
//! 修复后裸名（全仓 51 模块惯例：前端裸 cmd == match 臂字面量）。
//!
//! 断言手法：handle_cmd 执行序 = board service 检查（**第一行，先于
//! match**）→ match 臂。带真 board store 的 harness 下错误三态精确分层：
//! ①裸名穿过 board 闸命中臂体 → 空 store 查无此触点「触点 1 不存在」；
//! ②全名落 unknown 分支 →「未知 workbench 命令」；③「board service not
//! available」= 没进本模块逻辑（无 board 的装配态）。三态互斥，形态契约
//! 可机器锁死（board=None 的 harness 无法区分臂体与 unknown——检查先于
//! match，任何命令名都报同一错误）。

use super::workbench::WorkbenchHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_board::BoardStore;
use nemesis_types::cluster::NodeRole;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let store = BoardStore::open(&dir.path().join("board.db"), "NB").expect("open store");
    let service = nemesis_board::BoardService::new(Arc::new(store), NodeRole::Coordinator);
    let state = Arc::new(AppState {
        cors_manager: None,
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
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: Some(service),
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

/// 注册表清单 = 裸名形态（防回退全名——清单同时是 system.commands 与
/// wsapi-commands.md 的数据源）。
#[test]
fn commands_list_is_bare_names() {
    assert_eq!(WorkbenchHandler.commands(), &["get", "respond"]);
}

/// 裸名 get：穿过 board 闸 + 命中 match 臂体（空 store → 触点缺席）。
#[tokio::test]
async fn bare_get_hits_match_arm() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = WorkbenchHandler
        .handle_cmd(
            "get",
            Some(serde_json::json!({ "kind": "spec", "id": 1 })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("触点 1 不存在"),
        "裸名 get 应命中 match 臂体（错误应为空 store 触点缺席，实际：{err}）"
    );
}

/// 裸名 respond：同上（action 词表校验在触点查询之后，confirm 合法值
/// 穿透到查询层）。
#[tokio::test]
async fn bare_respond_hits_match_arm() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = WorkbenchHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "kind": "spec", "id": 1, "action": "confirm" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("触点 1 不存在"),
        "裸名 respond 应命中 match 臂体（错误应为空 store 触点缺席，实际：{err}）"
    );
}

/// 旧全名形态已退役：按全名调用落 unknown 分支（锁住「裸名=唯一 wire 契约」，
/// 防止有人把 match 臂改回全名或做「两边都认」的折中漂移）。
#[tokio::test]
async fn legacy_prefixed_names_fall_to_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    for cmd in ["workbench.get", "workbench.respond"] {
        let err = WorkbenchHandler
            .handle_cmd(cmd, Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(
            err.contains("未知 workbench 命令"),
            "全名 {cmd} 应落 unknown 分支，实际：{err}"
        );
    }
}

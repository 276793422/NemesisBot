//! api_handlers.rs AGT 覆盖率批次（2026-09-25）。与 s10b / fork_route 两套
//! 互补，聚焦仍缺的确定性臂（直调私有函数 / handler，免 axum 路由壳）：
//! - find_latest_request_summary 顶层**非目录**项的 continue 臂（s10b 只种
//!   目录，普通文件项从未进过循环跳过分支）
//! - read_log_entries 非法 UTF-8 读失败兜底（read_to_string 对含 0xFF 的
//!   文件必败，Windows/Unix 同构）
//! - /api/internal `shutdown` 的 mpsc 发送失败 → 500（rx 预先 drop）
//! - resolve_fork_store 活 agent 店分支（agent_loop Some + session_store
//!   Some → 直接返回活店，不走 fallback）——真 AgentLoop + 哑 Provider
//! - fork 对「非空但零 user 轮次」日志的 500 诚实上抛（404 预检只看行数，
//!   fork_session 的轮次检查独立失败）
//!
//! 结构性豁免（见报告）：
//! - 596 / 733 / 856 三个收括号行：lcov 归因伪零——所属 if-let 体内全部
//!   行均有正计数（593-594=16、717-731=3..6、854=8），体执行而括号行永不
//!   计数，属 llvm-cov 收括号归因缺口，任何测试都无法改变。
//! - 781 seek 失败臂：File::open 成功且 size>0 后对常规文件 seek(End)
//!   无平台无关失败手段，纯防御臂。

use super::*;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_make_state(
    dir: &tempfile::TempDir,
    internal_cmd_tx: Option<tokio::sync::mpsc::Sender<crate::internal::InternalCommand>>,
) -> Arc<AppState> {
    let ws = dir.path().to_string_lossy().to_string();
    Arc::new(AppState {
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
        // AppState 双声明（workflow 门控真型 / `Arc<()>` 桩），按 combo 装配
        // 保证无 workflow feature 也编译。
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
        internal_cmd_tx,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    })
}

fn agt_unique_sid() -> String {
    format!(
        "agtapiv{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

// ---------------------------------------------------------------------------
// find_latest_request_summary：顶层非目录项 continue
// ---------------------------------------------------------------------------

#[test]
fn agt_find_latest_skips_nondir_top_level_entries() {
    let dir = tempfile::tempdir().unwrap();
    // 唯一的真目录（内含一份 md）+ 顶层普通文件（非目录，触发 continue）。
    std::fs::create_dir_all(dir.path().join("B")).unwrap();
    std::fs::write(dir.path().join("B").join("b.md"), "b").unwrap();
    std::fs::write(dir.path().join("stray.txt"), "not a request dir").unwrap();

    let got = find_latest_request_summary(dir.path()).expect("目录里有 md");
    assert!(got.ends_with("b.md"), "got {got}");
}

// ---------------------------------------------------------------------------
// read_log_entries：非法 UTF-8 → 读失败兜底
// ---------------------------------------------------------------------------

#[test]
fn agt_read_log_entries_invalid_utf8_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bad.jsonl");
    let mut bytes = b"{\"id\":1}\n".to_vec();
    bytes.push(0xFF); // 非法 UTF-8 字节
    bytes.extend_from_slice(b"\n");
    std::fs::write(&p, bytes).unwrap();

    // size>0、seek 成功，read_to_string 必败 → 空 vec。
    assert!(read_log_entries(p.to_str().unwrap(), 5).is_empty());
}

// ---------------------------------------------------------------------------
// /api/internal：shutdown 发送失败 → 500
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_internal_shutdown_send_fail_maps_to_500() {
    let dir = tempfile::tempdir().unwrap();
    // rx 预先 drop：send 必败（闭路通道）。
    let (tx, rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(4);
    drop(rx);
    let state = agt_make_state(&dir, Some(tx));

    let resp = handle_api_internal(
        axum::http::HeaderMap::new(),
        axum::extract::State(state),
        axum::Json(serde_json::json!({ "cmd": "shutdown" })),
    )
    .await;
    let (status, body) = resp.expect_err("发送失败必须 Err");
    assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body.0["error"], "send failed", "{}", body.0);
}

// ---------------------------------------------------------------------------
// resolve_fork_store：活 agent 店分支
// ---------------------------------------------------------------------------

/// 哑 Provider：只供 AgentLoop 构造，任何调用都是测试缺陷。
struct AgtNoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for AgtNoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        unreachable!("dummy provider must not be called")
    }
}

#[tokio::test]
async fn agt_resolve_fork_store_prefers_live_agent_store() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("workspace").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    let mut al = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(AgtNoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    );
    let live = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
        &sessions,
    ));
    al.set_session_store(live.clone());

    let state = agt_make_state(&dir, None);
    *state.agent_loop.write() = Some(Arc::new(al));

    // 活 agent 分支：直接返回活店（不再走 <home>/workspace/sessions fallback）。
    match resolve_fork_store(&state) {
        Ok(got) => assert!(Arc::ptr_eq(&got, &live), "必须返回活 agent 的同一个店实例"),
        Err((status, body)) => panic!("活店必须解析成功: {status} {}", body.0),
    }
}

// ---------------------------------------------------------------------------
// fork：非空但零 user 轮次 → 500 诚实上抛
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_fork_assistant_only_log_maps_to_500() {
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let sid = agt_unique_sid();
    let key = chat_session_key(&sid);
    // 非空但没有任何 role=user 行：404 预检（只看行数）放行，
    // fork_session 的轮次检查独立失败 → 500。
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "只有回答一");
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "只有回答二");

    let state = agt_make_state(&dir, None);
    let resp = handle_api_chat_session_fork(
        axum::http::HeaderMap::new(),
        axum::extract::Path(sid.clone()),
        axum::extract::State(state),
        axum::Json(serde_json::json!({})),
    )
    .await;
    match resp {
        Err((status, body)) => {
            assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            let err = body.0["error"].as_str().unwrap().to_string();
            assert!(
                err.contains("没有任何完整 user 轮次"),
                "fork_session 的轮次错误必须透传: {err}"
            );
        }
        Ok(v) => panic!("零 user 轮次的日志必须 fork 失败: {}", v.0),
    }

    // 清理全局目录产物（nanos 唯一 key）。
    nemesis_agent::chat_log::delete_chat_log(&key);
}

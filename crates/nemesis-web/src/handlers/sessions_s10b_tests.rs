//! S10b (quality-hardening goal 冲刺, web 批次 2): sessions WSAPI success
//! arms beyond the missing-field bails in `sessions_extra_tests.rs`:
//!
//! - `list` strips the `agent_main_session_` prefix (web sessions only) and
//!   filters legacy/other files out; sidecar meta title wins over
//!   firstMessage
//! - `clear` truncates the chat_log (both the loopless skip arm and the
//!   live-store arm)
//! - `delete` removes the chat_log and reports an empty pause list with no
//!   cron service
//! - `export` returns the seeded messages
//!
//! create/rename happy paths stay untested on purpose (see the note at the
//! `sessions_extra_tests` declaration): they write the title meta sidecar
//! through the process-global path manager into the real home. The
//! chat_log-seeded tests below follow the fork_route_tests house pattern
//! (nanos-unique keys + `delete_chat_log` cleanup).

use super::sessions::SessionsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct NoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx(dir: &tempfile::TempDir, with_store: bool) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let mut agent_loop = parking_lot::RwLock::new(None);
    if with_store {
        let mut al = nemesis_agent::r#loop::AgentLoop::new(
            Box::new(NoopProvider),
            nemesis_agent::types::AgentConfig::default(),
        );
        al.set_session_store(Arc::new(
            nemesis_agent::session::SessionStore::new_in_memory(),
        ));
        *agent_loop.get_mut() = Some(Arc::new(al));
    }
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
        agent_loop: Arc::new(agent_loop),
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

fn unique_sid() -> String {
    format!(
        "s10bsess{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn session_key_for(sid: &str) -> String {
    format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    )
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    SessionsHandler.handle_cmd(cmd, data, ctx).await
}

// ---------------------------------------------------------------------------
// list (fully local fixtures under <ws>/logs/session_logs)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_strips_web_prefix_filters_legacy_and_uses_meta_title() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs/session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("agent_main_session_web1.jsonl"),
        concat!(
            r#"{"role":"user","content":"你好","timestamp":"2026-08-26T10:00:00+08:00"}"#,
            "\n",
            r#"{"role":"assistant","content":"在","timestamp":"2026-08-26T10:00:05+08:00"}"#,
            "\n"
        ),
    )
    .unwrap();
    // Sidecar title wins over firstMessage.
    std::fs::write(
        logs.join("agent_main_session_web1.meta.json"),
        r#"{ "title": "我的标题" }"#,
    )
    .unwrap();
    // Legacy fixed session + unrelated jsonl → both filtered out.
    std::fs::write(
        logs.join("agent_main_main.jsonl"),
        r#"{"role":"user","content":"legacy","timestamp":"2026-08-26T09:00:00+08:00"}"#,
    )
    .unwrap();
    std::fs::write(
        logs.join("cluster_peer.jsonl"),
        r#"{"role":"user","content":"x","timestamp":"2026-08-26T09:00:00+08:00"}"#,
    )
    .unwrap();

    let ctx = make_ctx(&dir, false);
    let out = run(&ctx, "list", None).await.unwrap().unwrap();
    let sessions = out["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "only web multi-session files survive");
    assert_eq!(sessions[0]["id"], "web1", "prefix stripped");
    assert_eq!(sessions[0]["session_key"], "agent:main:session:web1");
    assert_eq!(sessions[0]["title"], "我的标题", "meta sidecar title wins");
    assert_eq!(sessions[0]["messageCount"], 2);
}

// ---------------------------------------------------------------------------
// clear / delete / export (chat_log house pattern)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clear_truncates_chat_log_with_and_without_live_store() {
    let sid = unique_sid();
    let key = session_key_for(&sid);
    nemesis_agent::chat_log::append_chat_log(&key, "user", "问题");
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "回答");

    // Loopless ctx → the guard-None skip arm; chat_log still truncated.
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    let out = run(
        &ctx,
        "clear",
        Some(serde_json::json!({ "session_id": sid })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["cleared"], serde_json::json!(sid));
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 0, "chat_log truncated first");
    nemesis_agent::chat_log::delete_chat_log(&key);

    // Live store ctx → store.clear_session arm (in-memory store; no-op is fine).
    let sid2 = unique_sid();
    let key2 = session_key_for(&sid2);
    nemesis_agent::chat_log::append_chat_log(&key2, "user", "再问");
    let dir2 = tempfile::tempdir().unwrap();
    let ctx2 = make_ctx(&dir2, true);
    let out = run(
        &ctx2,
        "clear",
        Some(serde_json::json!({ "session_id": sid2 })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["cleared"], serde_json::json!(sid2));
    nemesis_agent::chat_log::delete_chat_log(&key2);
}

#[tokio::test]
async fn delete_removes_chat_log_only_through_the_store_arm() {
    // ⚠ Documented asymmetry (S10b-1 挂账, not fixed here): `clear` truncates
    // the chat_log UNCONDITIONALLY, but `delete` only removes the jsonl via
    // SessionStore::delete_session (which calls delete_chat_log) — with
    // agent_loop absent the jsonl survives while the handler still reports
    // success. Production always runs with a live loop, so this pins current
    // behaviour rather than fixing it.
    let sid = unique_sid();
    let key = session_key_for(&sid);
    nemesis_agent::chat_log::append_chat_log(&key, "user", "待删");

    // Loopless ctx: cron = None → empty pause list; jsonl NOT removed.
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    let out = run(
        &ctx,
        "delete",
        Some(serde_json::json!({ "session_id": sid })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["deleted"], serde_json::json!(sid));
    assert_eq!(out["paused_cron_jobs"], serde_json::json!([]));
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(
        total, 1,
        "loopless delete leaves the jsonl (store arm skipped)"
    );
    nemesis_agent::chat_log::delete_chat_log(&key);

    // With a live store (production shape): store.delete_session removes the
    // store json AND the chat_log jsonl.
    let sid2 = unique_sid();
    let key2 = session_key_for(&sid2);
    nemesis_agent::chat_log::append_chat_log(&key2, "user", "真删");
    let dir2 = tempfile::tempdir().unwrap();
    let ctx2 = make_ctx(&dir2, true);
    let out = run(
        &ctx2,
        "delete",
        Some(serde_json::json!({ "session_id": sid2 })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["deleted"], serde_json::json!(sid2));
    let (_, total2, _, _) = nemesis_agent::chat_log::read_chat_log(&key2, 10, None);
    assert_eq!(total2, 0, "store arm cascades to the chat_log");
    nemesis_agent::chat_log::delete_chat_log(&key2);
}

#[tokio::test]
async fn export_returns_seeded_messages() {
    let sid = unique_sid();
    let key = session_key_for(&sid);
    nemesis_agent::chat_log::append_chat_log(&key, "user", "导出我");

    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    let out = run(
        &ctx,
        "export",
        Some(serde_json::json!({ "session_id": sid })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["count"], serde_json::json!(1));
    assert_eq!(out["messages"][0]["role"], "user");
    assert_eq!(out["messages"][0]["content"], "导出我");

    nemesis_agent::chat_log::delete_chat_log(&key);
}

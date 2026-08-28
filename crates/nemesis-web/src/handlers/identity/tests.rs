//! G5 (U18)：identity handler 的 AGENTS.md/CLAUDE.md 指令链文档。
//!
//! 钉三件事：list 覆盖六件套且 instruction_chain 标志正确；save→get 对
//! 新指令链文件可创建/回读；缺失文件的 get 保持既有语义（报错，不返回空）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[test]
fn list_covers_six_docs_and_flags_instruction_chain() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // 两个既有文档落盘，其余保持缺失。
    std::fs::write(dir.path().join("AGENT.md"), "# Agent\n").unwrap();
    std::fs::write(dir.path().join("IDENTITY.md"), "# Identity\n").unwrap();

    let out = IdentityHandler.list(ctx.workspace.as_deref().unwrap())
        .unwrap()
        .unwrap();
    let docs = out["documents"].as_array().unwrap();
    let names: Vec<&str> = docs
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["AGENT.md", "IDENTITY.md", "SOUL.md", "USER.md", "AGENTS.md", "CLAUDE.md"]
    );

    let by_name = |n: &str| docs.iter().find(|d| d["name"] == n).unwrap();
    assert_eq!(by_name("AGENT.md")["exists"], true);
    assert_eq!(by_name("SOUL.md")["exists"], false);
    assert_eq!(by_name("SOUL.md")["size"], 0);

    // 指令链标志只落在 AGENTS.md / CLAUDE.md 上。
    for n in ["AGENT.md", "IDENTITY.md", "SOUL.md", "USER.md"] {
        assert_eq!(by_name(n)["instruction_chain"], false, "{}", n);
    }
    for n in ["AGENTS.md", "CLAUDE.md"] {
        assert_eq!(by_name(n)["instruction_chain"], true, "{}", n);
        assert_eq!(by_name(n)["exists"], false);
    }
}

#[test]
fn save_then_get_roundtrip_for_new_chain_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = ctx.workspace.as_deref().unwrap().to_string();

    IdentityHandler
        .save(&ws, "AGENTS.md", "# 指令链\n不要乱删文件")
        .unwrap()
        .unwrap();
    // 盘上真实存在（写的是 workspace 根，不是别的目录）。
    let on_disk = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(on_disk.contains("不要乱删文件"));

    let got = IdentityHandler.get(&ws, "AGENTS.md").unwrap().unwrap();
    assert_eq!(got["content"], "# 指令链\n不要乱删文件");

    // list 回读 exists/size 同步。
    let out = IdentityHandler.list(&ws).unwrap().unwrap();
    let agents = out["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "AGENTS.md")
        .unwrap();
    assert_eq!(agents["exists"], true);
    assert!(agents["size"].as_u64().unwrap() > 0);
}

#[test]
fn get_missing_doc_still_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = ctx.workspace.as_deref().unwrap().to_string();
    // 既有语义：缺失文件的 get 报错（前端按 list 的 exists 标志规避）。
    let err = IdentityHandler.get(&ws, "CLAUDE.md").unwrap_err();
    assert!(err.contains("failed to read"), "err={err}");
}
